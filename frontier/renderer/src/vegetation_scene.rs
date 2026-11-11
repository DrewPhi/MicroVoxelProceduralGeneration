//! Scene integration: canonical plants and a bounded, independently streamed display layer.
use super::*;
impl Gpu {
    pub(super) fn settle_naadf(&mut self, world: &World, camera: Camera) -> Result<()> {
        if self
            .visual_settings
            .as_ref()
            .is_none_or(|s| !s.naadf_enabled)
        {
            return Ok(());
        }
        let start = Instant::now();
        loop {
            self.update_naadf(world, camera);
            self.queue.submit(std::iter::empty());
            self.device.poll(wgpu::Maintain::Poll);
            let n = self.naadf.as_ref().unwrap();
            if n.ready_for_capture() {
                return Ok(());
            }
            if let Some(e) = &n.error {
                return Err(e.clone().into());
            }
            if start.elapsed().as_secs() > 300 {
                return Err("NAADF snapshot did not finish within 300s".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    pub(super) fn update_naadf(&mut self, world: &World, camera: Camera) {
        if let Some(stages) = &self.naadf_gpu_stages {
            stages.poll(&self.device);
        }
        if self.naadf.is_none()
            && self
                .visual_settings
                .as_ref()
                .is_none_or(|s| !s.naadf_enabled)
        {
            return;
        }
        let mut experiment = self.naadf.take().unwrap_or_default();
        experiment.poll(
            self,
            world,
            camera,
            self.naadf_serial,
            self.vegetation_background_paused,
            self.naadf_raster_restored,
        );
        let bindings = experiment.shadow_bindings();
        if let Some(adaptive) = &mut self.adaptive {
            adaptive.set_naadf_shadow(&self.device, bindings);
            adaptive.set_naadf_shadow_bounds(experiment.full_bounds());
        }
        self.naadf = Some(experiment);
    }
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
            self.vegetation_inspection.clear();
            self.vegetation_caster_roots.clear();
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
        if let Some(stream) = &mut self.vegetation_stream {
            stream.set_naadf_owner(self.naadf.as_ref().and_then(|n| n.desired_owner()));
            stream.set_naadf_retirement(self.naadf.as_ref().and_then(|n| n.retirement()));
        }
        // Adaptive ground owns separate tile-relative uniforms. Its empty
        // native mesh lets the plant pass use a moving integer-aligned origin.
        if self.adaptive.is_some() && !self.volume_owner && self.index_count == 0 {
            self.origin =
                DVec3::from_array(camera.position.to_array().map(|v| (v / 32.).floor() * 32.));
        }
        if let Some(settings) = &self.visual_settings {
            let stream = self.vegetation_stream.as_mut().unwrap();
            stream.set_visibility_benchmark(
                settings.visibility_benchmark,
                settings.naadf_radius_m,
                camera.position.to_array(),
            );
            stream.set_shadows_enabled(
                settings.beauty && settings.terrain_shadows && !settings.visibility_benchmark,
            );
            self.vegetation_stream
                .as_mut()
                .unwrap()
                .configure_resources(
                    settings.plant_cpu_mib.saturating_mul(1024 * 1024),
                    settings.plant_gpu_mib.saturating_mul(1024 * 1024),
                    settings.triangle_budget,
                    settings.vegetation_near_m,
                );
        }
        self.vegetation_stream
            .as_mut()
            .unwrap()
            .set_background_paused(self.vegetation_background_paused);
        let frame = self.vegetation_stream.as_mut().unwrap().update_view(
            camera.position.to_array(),
            height,
            camera.fov_y.to_radians(),
            world.revision(),
            camera.direction().as_dvec3().to_array(),
            width as f32 / height.max(1) as f32,
        );
        self.vegetation_stats = frame.stats;
        self.vegetation_inspection
            .clone_from(&frame.shadow_inspection);
        if self.vegetation.is_none() || self.vegetation_samples != self.sample_count {
            self.vegetation = Some(vegetation_gpu::VegetationGpu::new(
                &self.device,
                self.scene_format,
                self.sample_count,
                &self.native_layout,
            ));
            self.vegetation
                .as_mut()
                .unwrap()
                .set_indirect_supported(self.vegetation_indirect_supported);
            self.vegetation_samples = self.sample_count;
        }
        let plants = self.vegetation.as_mut().unwrap();
        plants.set_color_view_projection(
            camera
                .matrix(self.origin, width as f32 / height.max(1) as f32)
                .to_cols_array_2d(),
        );
        if let Some(settings) = &self.visual_settings {
            plants.set_visual_lighting(&self.queue, settings, self.regional_appearance);
        }
        let origin = self
            .origin
            .to_array()
            .map(|v| (v / VOXEL_SIZE).round() as i64);
        if let Some(stream) = &self.vegetation_stream {
            plants.discard_naadf_templates(&stream.naadf_omitted_keys);
        }
        plants.update(
            &self.device,
            &self.queue,
            &frame,
            origin,
            (camera.position - self.origin).as_vec3().to_array(),
        )?;
        self.vegetation_caster_roots = plants.shadow_caster_roots();
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
            if self.vegetation_stats.pending_jobs == 0
                && self.vegetation_stats.shadow_transition_pairs == 0
                && self.vegetation_stats.shadow_fading_casters == 0
            {
                if self.vegetation_stats.requested_native_unresolved > 0
                    || self.vegetation_stats.coverage_unresolved > 0
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
        let mut json = format!("\"vegetation\":{{\"enabled\":{},\"source_version\":{},\"instance_units\":\"ground_tiles_and_individual_trees\",\"model_generation\":\"full_record_seed_rules\",\"resident_tiles\":{},\"resident_templates\":{},\"pending_jobs\":{},\"missing_tiles\":{},\"missing_templates\":{},\"selected_instances\":{},\"triangles\":{},\"budget_rejected\":{},\"culled_instances\":{},\"coverage_fallbacks\":{},\"coverage_unresolved\":{},\"selected_by_species\":{:?},\"triangles_by_species\":{:?},\"rejected_by_species\":{:?},\"draw_calls\":{},\"gpu_bytes\":{},\"uploaded_bytes_last_update\":{},\"grass_radius_m\":28,\"flower_radius_m\":56,\"tree_radius_m\":220,\"shadow_casters\":{},\"shadow_instances\":{},\"shadow_triangles\":{},\"reference_mode\":{},\"reference_budget_exceeded\":{},\"cpu_geometry_budget_bytes\":{},\"gpu_budget_bytes\":{},\"gpu_actual_budget_bytes\":{},\"shadow_only_instances\":{},\"shadow_only_triangles\":{},\"retained_ground_detail\":{},\"canonical_voxel_m\":0.0625}}",
            self.vegetation.is_some(),frontier_world::vegetation::VEGETATION_SOURCE_VERSION,s.resident_tiles,s.resident_meshes,s.pending_jobs,s.missing_tiles,s.missing_meshes,s.selected_instances,s.triangles,s.budget_rejected,s.culled_instances,s.coverage_fallbacks,s.coverage_unresolved,s.selected_by_species,s.triangles_by_species,s.rejected_by_species,g.draw_calls,g.resident_bytes,g.uploaded_bytes,self.adaptive.as_ref().is_some_and(|a|a.near_shadows_active()),g.shadow_instances,g.shadow_triangles,self.vegetation_stream.as_ref().is_some_and(|s|s.config.reference),s.reference_budget_exceeded,s.mesh_budget_bytes,s.gpu_budget_bytes,g.cache_budget_bytes,s.shadow_only_instances,s.shadow_only_triangles,s.retained_ground_detail);
        json.pop();
        json.push_str(&format!(
            ",\"naadf_omitted_groups\":{},\"requested_native_unresolved\":{},\"naadf_retire_unresolved\":{}}}",
            self.vegetation_stream
                .as_ref()
                .map_or(0, |s| s.naadf_omitted),
            self.vegetation_stats.requested_native_unresolved,
            self.vegetation_stats.naadf_retire_unresolved
        ));
        json.pop();
        let j = s.jobs;
        json.push_str(&format!(",\"gpu_representation\":\"{}\",\"tree_lod_changes\":{},\"tree_refinements\":{},\"tree_coarsenings\":{},\"tree_lod_histogram\":{:?},\"demanded_by_species\":{:?},\"ready_by_species\":{:?},\"shadow_admitted\":{},\"shadow_removed\":{},\"shadow_budget_rejected\":{},\"shadow_stream_budget_rejected\":{},\"tree_shadow_triangle_budget\":150000,\"total_shadow_triangle_budget\":200000,\"explicit_shadow_instances\":{},\"explicit_shadow_triangles\":{},\"shadow_lod_histogram\":{:?},\"shadow_lod_changes\":{},\"selected_cpu_geometry_bytes\":{},\"total_cpu_geometry_bytes\":{},\"cpu_cache_geometry_bytes\":{},\"selected_cpu_meshes\":{},\"selected_mesh_evictions\":{},\"selected_mesh_evicted_bytes\":{},\"ground_assembly_cache\":{},\"jobs\":{{\"workers\":{},\"started\":{:?},\"completed\":{:?},\"cancelled\":{:?},\"worker_wall_nanos\":{:?},\"max_job_nanos\":{:?},\"assembly_nanos\":{:?},\"mesh_nanos\":{:?},\"scope\":\"cumulative overlapping worker wall time; order placement,tree,ground; assembly includes growth/cache lookups and native-cell assembly\"}}}}", self.vegetation.as_ref().map_or("disabled", |v|v.representation()),s.tree_lod_changes,s.tree_refinements,s.tree_coarsenings,s.tree_lod_histogram,s.demanded_by_species,s.ready_by_species,g.shadow_admitted,g.shadow_removed,g.shadow_budget_rejected,s.shadow_budget_rejected,s.explicit_shadow_instances,s.explicit_shadow_triangles,s.shadow_lod_histogram,s.shadow_lod_changes,s.selected_cpu_geometry_bytes,s.total_cpu_geometry_bytes,s.cpu_cache_geometry_bytes,s.selected_cpu_meshes,s.selected_mesh_evictions,s.selected_mesh_evicted_bytes,vegetation::ground_cache_json(),j.workers,j.started,j.completed,j.cancelled,j.cpu_nanos,j.max_job_nanos,j.assembly_nanos,j.mesh_nanos));
        json.pop();
        json.push_str(&format!(",\"shadow_transition_pairs\":{},\"shadow_transition_triangles\":{},\"shadow_fading_casters\":{},\"prediction_horizon_ms\":{},\"prediction_lead_m\":{},\"predicted_tiles\":{},\"pooled_reserved_bytes\":{},\"pooled_used_bytes\":{},\"cull_clusters_submitted\":{},\"gpu_frustum_culling\":{}}}", s.shadow_transition_pairs,s.shadow_transition_triangles,s.shadow_fading_casters,s.prediction_horizon_ms,s.prediction_lead_m,s.predicted_tiles,g.pooled_reserved_bytes,g.pooled_used_bytes,g.cull_clusters,g.cull_enabled));
        json.pop();
        json.push_str(&format!(",\"shadow_lifecycle_enabled\":{},\"near_shadow_cache_reused\":{},\"far_shadow_cache_reused\":{},\"shadow_cascades_enabled\":{}}}",
            s.shadow_lifecycle_enabled,
            self.adaptive.as_ref().is_some_and(|a| a.near_shadow_cache_reused()),
            self.adaptive.as_ref().is_some_and(|a| a.shadow_cache_reused()),
            self.adaptive.as_ref().is_some_and(|a| a.shadow_cascades_active())));
        // Format diagnostics only when a report is requested. Streaming keeps a
        // bounded in-memory sample and does no per-patch hot-path I/O.
        let ground_inspection = self.vegetation_stream.as_ref().map_or_else(String::new, |stream| {
            stream.ground_inspection().iter().map(|patch| {
                let selected = patch.selected_lod.map_or_else(|| "null".to_owned(), |lod| lod.to_string());
                format!("{{\"species\":{},\"tile\":{:?},\"seed\":{},\"edit_id\":{},\"distance_m\":{},\"target_lod\":{},\"selected_lod\":{},\"triangles\":{},\"headroom\":{},\"reason\":\"{}\"}}",
                    patch.key.species, patch.key.tile, patch.key.seed, patch.key.edit_id,
                    patch.distance_m, patch.target_lod, selected, patch.triangles, patch.headroom, patch.reason)
            }).collect::<Vec<_>>().join(",")
        });
        json.pop();
        json.push_str(&format!(",\"ground_decisions\":{{\"selected\":{},\"missing_mesh\":{},\"budget_rejected\":{},\"retained\":{},\"coarse_fallback\":{}}},\"ground_inspection_limit\":256,\"ground_inspection\":[{}]}}",
            s.ground_decisions[0],s.ground_decisions[1],s.ground_decisions[2],s.ground_decisions[3],s.ground_decisions[4],ground_inspection));
        json
    }
}
