//! Scene integration: canonical plants and a bounded, independently streamed display layer.
use super::*;
impl Gpu {
    pub(super) fn update_vegetation(
        &mut self,
        world: &World,
        camera: Camera,
        width: u32,
        height: u32,
    ) -> Result<()> {
        self.medium_ground_height = world.height_m(camera.position.x, camera.position.z) as f32;
        if !world.vegetation_enabled() {
            self.vegetation.take();
            self.vegetation_stream.take();
            self.vegetation_stats = Default::default();
            return Ok(());
        }
        if self.vegetation_stream.is_none() {
            self.vegetation_stream = Some(vegetation::VegetationStream::new(
                world.clone(),
                Default::default(),
            ));
            self.vegetation_revision = world.revision();
        } else if self.vegetation_revision != world.revision() {
            self.vegetation_stream
                .as_mut()
                .unwrap()
                .set_world(world.clone());
            self.vegetation_revision = world.revision();
        }
        // Adaptive ground owns separate tile-relative uniforms. Its empty
        // native mesh lets the plant pass use a moving integer-aligned origin.
        if self.adaptive.is_some() && !self.volume_owner && self.index_count == 0 {
            self.origin =
                DVec3::from_array(camera.position.to_array().map(|v| (v / 32.).floor() * 32.));
        }
        let frame = self.vegetation_stream.as_mut().unwrap().update_view(
            camera.position.to_array(),
            height,
            camera.fov_y.to_radians(),
            world.revision(),
            camera.direction().as_dvec3().to_array(),
            width as f32 / height.max(1) as f32,
        );
        self.vegetation_stats = frame.stats;
        if self.vegetation.is_none() || self.vegetation_samples != self.sample_count {
            self.vegetation = Some(vegetation_gpu::VegetationGpu::new(
                &self.device,
                self.scene_format,
                self.sample_count,
                &self.native_layout,
            ));
            self.vegetation_samples = self.sample_count;
        }
        let plants = self.vegetation.as_mut().unwrap();
        if let Some(settings) = &self.visual_settings {
            plants.set_visual_lighting(&self.queue, settings, self.regional_appearance);
        }
        let origin = self
            .origin
            .to_array()
            .map(|v| (v / VOXEL_SIZE).round() as i64);
        plants.update(
            &self.device,
            &self.queue,
            &frame,
            origin,
            (camera.position - self.origin).as_vec3().to_array(),
        )?;
        Ok(())
    }
    pub(super) fn settle_vegetation(
        &mut self,
        world: &World,
        camera: Camera,
        width: u32,
        height: u32,
    ) -> Result<()> {
        let started = Instant::now();
        loop {
            self.update_vegetation(world, camera, width, height)?;
            if self.vegetation_stats.pending_jobs == 0 {
                if self.vegetation_stats.coverage_unresolved > 0
                    || self.vegetation_stats.reference_budget_exceeded
                {
                    return Err(format!(
                        "vegetation tree coverage exceeds its resource budget: {}",
                        self.vegetation_json()
                    )
                    .into());
                }
                return Ok(());
            }
            if started.elapsed().as_secs() >= 60 {
                return Err(format!(
                    "vegetation preparation timed out: {}",
                    self.vegetation_json()
                )
                .into());
            }
            // Flush staging writes while waiting; captures must not accumulate
            // every intermediate instance upload until their final submit.
            self.queue.submit(std::iter::empty());
            self.device.poll(wgpu::Maintain::Poll);
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
    }
    pub(super) fn vegetation_json(&self) -> String {
        let s = self.vegetation_stats;
        let g = self
            .vegetation
            .as_ref()
            .map(|v| v.stats())
            .unwrap_or_default();
        format!("\"vegetation\":{{\"enabled\":{},\"source_version\":{},\"instance_units\":\"ground_tiles_and_individual_trees\",\"model_generation\":\"full_record_seed_rules\",\"resident_tiles\":{},\"resident_templates\":{},\"pending_jobs\":{},\"missing_tiles\":{},\"missing_templates\":{},\"selected_instances\":{},\"triangles\":{},\"budget_rejected\":{},\"culled_instances\":{},\"coverage_fallbacks\":{},\"coverage_unresolved\":{},\"selected_by_species\":{:?},\"triangles_by_species\":{:?},\"rejected_by_species\":{:?},\"draw_calls\":{},\"gpu_bytes\":{},\"uploaded_bytes_last_update\":{},\"grass_radius_m\":28,\"flower_radius_m\":56,\"tree_radius_m\":220,\"shadow_casters\":{},\"shadow_instances\":{},\"shadow_triangles\":{},\"reference_mode\":{},\"reference_budget_exceeded\":{},\"cpu_geometry_budget_bytes\":{},\"gpu_budget_bytes\":{},\"gpu_actual_budget_bytes\":{},\"shadow_only_instances\":{},\"shadow_only_triangles\":{},\"retained_ground_detail\":{},\"canonical_voxel_m\":0.0625}}",
            self.vegetation.is_some(),frontier_world::vegetation::VEGETATION_SOURCE_VERSION,s.resident_tiles,s.resident_meshes,s.pending_jobs,s.missing_tiles,s.missing_meshes,s.selected_instances,s.triangles,s.budget_rejected,s.culled_instances,s.coverage_fallbacks,s.coverage_unresolved,s.selected_by_species,s.triangles_by_species,s.rejected_by_species,g.draw_calls,g.resident_bytes,g.uploaded_bytes,self.adaptive.as_ref().is_some_and(|a|a.near_shadows_active()),g.shadow_instances,g.shadow_triangles,self.vegetation_stream.as_ref().is_some_and(|s|s.config.reference),s.reference_budget_exceeded,s.mesh_budget_bytes,s.gpu_budget_bytes,g.cache_budget_bytes,s.shadow_only_instances,s.shadow_only_triangles,s.retained_ground_detail)
    }
}
