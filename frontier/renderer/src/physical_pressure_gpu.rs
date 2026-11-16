//! Preparatory #88 GPU pressure component, independent of physical-fluid M1.
//! Greenfield Jacobi-PCG following Bridson/Müller-Fischer SIGGRAPH2007 ch4:
//! https://www.cs.ubc.ca/~rbridson/fluidsimulation/fluids_notes.pdf
//! No fluid topology, ghost treatment, timestep or runtime integration is supplied.
use bytemuck::{Pod, Zeroable};
use std::{collections::VecDeque, sync::mpsc};
use wgpu::util::DeviceExt;
const SHADER: &str = include_str!("physical_pressure_gpu.wgsl");
pub const MAX_ROWS: usize = 64 * 64 * 64;
pub const MISSING: u32 = u32::MAX;
const WORKGROUP: usize = 256;
/// Row of A: positive diagonal and up to six unit negative off-diagonals.
/// Missing links represent boundary elimination; caller supplies its adjusted RHS.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct PressureRow {
    pub neighbors: [u32; 6],
    pub diagonal: f32,
    pub rhs: f32,
}
#[derive(Clone, Copy, Debug)]
pub struct PressureConfig {
    pub max_iterations: u32,
    pub relative_tolerance: f32,
    pub absolute_tolerance: f32,
    /// Includes resident buffers and optional final diagnostic readback staging.
    pub byte_budget: u64,
}
impl Default for PressureConfig {
    fn default() -> Self {
        Self {
            max_iterations: 256,
            relative_tolerance: 1e-5,
            absolute_tolerance: 1e-6,
            byte_budget: 32 * 1048576,
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Config {
    count: u32,
    groups: u32,
    max_iterations: u32,
    pad: u32,
    relative: f32,
    absolute: f32,
    pad1: f32,
    pad2: f32,
}
/// GPU status0active,1converged,2numerical breakdown,3iteration limit.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct PressureState {
    pub rho: f32,
    pub alpha: f32,
    pub beta: f32,
    pub residual_squared: f32,
    pub rhs_squared: f32,
    pub target_squared: f32,
    pub iterations: u32,
    pub status: u32,
}
#[derive(Debug)]
pub struct PressureDiagnostic {
    pub solution: Vec<f32>,
    pub state: PressureState,
}
/// Admission accepts the symmetric diagonally dominant M-matrix SPD subset.
/// Each connected component must have a strict diagonal row (a gauge or a
/// Dirichlet contribution). Singular pure Neumann components are rejected.
fn validate(rows: &[PressureRow], config: PressureConfig) -> Result<u64, String> {
    if rows.is_empty() || rows.len() > MAX_ROWS {
        return Err("pressure domain must contain1..64^3rows".into());
    }
    if !(1..=2048).contains(&config.max_iterations)
        || !config.relative_tolerance.is_finite()
        || !(1e-7..=0.1).contains(&config.relative_tolerance)
        || !config.absolute_tolerance.is_finite()
        || !(1e-12..=1.).contains(&config.absolute_tolerance)
    {
        return Err("invalid pressure iteration/tolerance budget".into());
    }
    for (i, row) in rows.iter().enumerate() {
        if !row.diagonal.is_finite() || !row.rhs.is_finite() || row.diagonal <= 0. {
            return Err("nonfinite/nonpositive pressure row".into());
        }
        let mut degree = 0;
        for (slot, &j) in row.neighbors.iter().enumerate() {
            if j == MISSING {
                continue;
            }
            if j as usize >= rows.len() || j as usize == i || row.neighbors[..slot].contains(&j) {
                return Err("invalid/duplicate pressure neighbor".into());
            }
            if !rows[j as usize].neighbors.contains(&(i as u32)) {
                return Err("pressure neighbor matrix is asymmetric".into());
            }
            degree += 1;
        }
        if row.diagonal < degree as f32 {
            return Err("pressure matrix is not diagonally dominant".into());
        }
    }
    let mut seen = vec![false; rows.len()];
    for start in 0..rows.len() {
        if seen[start] {
            continue;
        }
        let mut queue = VecDeque::from([start]);
        seen[start] = true;
        let mut anchored = false;
        while let Some(i) = queue.pop_front() {
            let row = rows[i];
            let degree = row.neighbors.iter().filter(|&&j| j != MISSING).count();
            anchored |= row.diagonal > degree as f32;
            for j in row.neighbors.into_iter().filter(|&j| j != MISSING) {
                let j = j as usize;
                if !seen[j] {
                    seen[j] = true;
                    queue.push_back(j);
                }
            }
        }
        if !anchored {
            return Err(
                "unanchored/singular pressure component; eliminate an explicit gauge".into(),
            );
        }
    }
    //32-byte input rows,16-byte vectors,16-byte block sums,32-byte scalar
    // state,32-byte uniform, plus a final16*N+32 diagnostic copy.
    let bytes = rows.len() as u64 * 64 + rows.len().div_ceil(WORKGROUP) as u64 * 16 + 96;
    if bytes > config.byte_budget {
        return Err("pressure buffer budget exceeded".into());
    }
    Ok(bytes)
}
pub struct PhysicalPressureGpu {
    vectors: wgpu::Buffer,
    state: wgpu::Buffer,
    bind: wgpu::BindGroup,
    pipelines: Vec<wgpu::ComputePipeline>,
    count: usize,
    groups: u32,
    max_iterations: u32,
    pub allocated_and_diagnostic_budget_bytes: u64,
}
impl PhysicalPressureGpu {
    pub fn new(
        device: &wgpu::Device,
        rows: &[PressureRow],
        config: PressureConfig,
    ) -> Result<Self, String> {
        let bytes = validate(rows, config)?;
        let limits = device.limits();
        let row_bytes = rows.len() as u64 * 32;
        let vector_bytes = rows.len() as u64 * 16;
        let largest_buffer = row_bytes.max(vector_bytes + 32).max(32);
        if row_bytes > limits.max_storage_buffer_binding_size as u64
            || largest_buffer > limits.max_buffer_size
            || vector_bytes > limits.max_storage_buffer_binding_size as u64
            || limits.max_uniform_buffer_binding_size < 32
            || limits.max_storage_buffers_per_shader_stage < 4
            || limits.max_compute_invocations_per_workgroup < 256
            || limits.max_compute_workgroup_size_x < 256
            || limits.max_compute_workgroup_storage_size < 4096
            || rows.len().div_ceil(WORKGROUP) > limits.max_compute_workgroups_per_dimension as usize
        {
            return Err("pressure buffers/workgroups exceed device limits".into());
        }
        let input = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("pressure rows"),
            contents: bytemuck::cast_slice(rows),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let vectors = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pressure x/r/p/Ap"),
            size: vector_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let state = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pressure GPU coefficients/status"),
            size: 32,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let groups = rows.len().div_ceil(WORKGROUP) as u32;
        let partial = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pressure block dot sums"),
            size: groups as u64 * 16,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("pressure budget/config"),
            contents: bytemuck::bytes_of(&Config {
                count: rows.len() as u32,
                groups,
                max_iterations: config.max_iterations,
                pad: 0,
                relative: config.relative_tolerance,
                absolute: config.absolute_tolerance,
                pad1: 0.,
                pad2: 0.,
            }),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let entries = (0..5)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: if binding == 4 {
                        wgpu::BufferBindingType::Uniform
                    } else {
                        wgpu::BufferBindingType::Storage {
                            read_only: binding == 0,
                        }
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect::<Vec<_>>();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pressure resident layout"),
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pressure pipeline layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pressure PCG shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let names = [
            "initialize",
            "initialize_sum",
            "multiply_direction",
            "alpha_sum",
            "update_solution",
            "true_residual",
            "beta_sum",
            "update_direction",
            "finalize",
        ];
        let pipelines = names
            .into_iter()
            .map(|name| {
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(name),
                    layout: Some(&pipeline_layout),
                    module: &module,
                    entry_point: Some(name),
                    compilation_options: Default::default(),
                    cache: None,
                })
            })
            .collect();
        let buffers = [&input, &vectors, &partial, &state, &uniform];
        let entries = buffers
            .iter()
            .enumerate()
            .map(|(binding, b)| wgpu::BindGroupEntry {
                binding: binding as u32,
                resource: b.as_entire_binding(),
            })
            .collect::<Vec<_>>();
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pressure resident buffers"),
            layout: &layout,
            entries: &entries,
        });
        Ok(Self {
            vectors,
            state,
            bind,
            pipelines,
            count: rows.len(),
            groups,
            max_iterations: config.max_iterations,
            allocated_and_diagnostic_budget_bytes: bytes,
        })
    }
    /// Encodes the entire fixed-budget solve. All reductions, coefficient
    /// updates, true-residual convergence and breakdown flags stay on the GPU.
    /// Completed/failed states gate subsequent dispatches; there is no CPU wait.
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("pressure fixed-budget PCG"),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, &self.bind, &[]);
        let dispatch = |pass: &mut wgpu::ComputePass<'_>, pipeline: usize, groups| {
            pass.set_pipeline(&self.pipelines[pipeline]);
            pass.dispatch_workgroups(groups, 1, 1);
        };
        dispatch(&mut pass, 0, self.groups);
        dispatch(&mut pass, 1, 1);
        for _ in 0..self.max_iterations {
            for (pipeline, groups) in [
                (2, self.groups),
                (3, 1),
                (4, self.groups),
                (5, self.groups),
                (6, 1),
                (7, self.groups),
            ] {
                dispatch(&mut pass, pipeline, groups);
            }
        }
        dispatch(&mut pass, 8, 1);
    }
    /// Resident16-byte row record; x is the first scalar. No implicit readback.
    pub fn solution_vectors(&self) -> &wgpu::Buffer {
        &self.vectors
    }
    /// Resident coefficient/status record, for future GPU-side success gating.
    /// A consumer must require status1 before using x as an accepted solution.
    pub fn state_buffer(&self) -> &wgpu::Buffer {
        &self.state
    }
    /// Explicit final diagnostic only; not intended inside a simulation loop.
    pub fn readback_diagnostic(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<PressureDiagnostic, String> {
        let vector_bytes = self.count as u64 * 16;
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pressure final diagnostic readback"),
            size: vector_bytes + 32,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_buffer_to_buffer(&self.vectors, 0, &read, 0, vector_bytes);
        encoder.copy_buffer_to_buffer(&self.state, 0, &read, vector_bytes, 32);
        queue.submit(Some(encoder.finish()));
        let (tx, rx) = mpsc::channel();
        read.slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        device.poll(wgpu::Maintain::Wait);
        rx.recv()
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let data = read.slice(..).get_mapped_range();
        let solution = bytemuck::cast_slice::<u8, [f32; 4]>(&data[..vector_bytes as usize])
            .iter()
            .map(|v| v[0])
            .collect();
        let state = bytemuck::pod_read_unaligned(&data[vector_bytes as usize..]);
        drop(data);
        read.unmap();
        Ok(PressureDiagnostic { solution, state })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shader_and_host_layouts_validate() {
        assert_eq!(std::mem::size_of::<PressureRow>(), 32);
        assert_eq!(std::mem::size_of::<Config>(), 32);
        assert_eq!(std::mem::size_of::<PressureState>(), 32);
        let module = naga::front::wgsl::parse_str(SHADER).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }
    fn row(diagonal: f32, neighbors: &[u32]) -> PressureRow {
        let mut out = PressureRow {
            diagonal,
            rhs: 0.,
            neighbors: [MISSING; 6],
        };
        out.neighbors[..neighbors.len()].copy_from_slice(neighbors);
        out
    }
    #[test]
    fn rejects_singular_asymmetric_nonfinite_and_over_budget_inputs() {
        let config = PressureConfig::default();
        assert!(validate(&[], config).is_err());
        assert!(validate(&[row(1., &[1]), row(1., &[0])], config).is_err());
        assert!(validate(&[row(2., &[1]), row(1., &[])], config).is_err());
        assert!(validate(&[row(3., &[1, 1]), row(1., &[0])], config).is_err());
        assert!(validate(&[row(f32::NAN, &[])], config).is_err());
        assert!(validate(
            &[row(1., &[])],
            PressureConfig {
                byte_budget: 1,
                ..config
            }
        )
        .is_err());
        assert!(validate(&[row(2., &[1]), row(1., &[0])], config).is_ok());
        assert!(validate(&[row(1., &[]), row(1., &[])], config).is_ok());
    }
    fn fixture(d: [usize; 3], kind: u8) -> (Vec<PressureRow>, Vec<f64>) {
        let n = d.iter().product();
        let index = |p: [usize; 3]| (p[2] * d[1] + p[1]) * d[0] + p[0];
        let obstacle = |p: [usize; 3]| kind == 2 && p[0] == d[0] / 2;
        let pin =
            |p: [usize; 3]| kind > 0 && (p == [0, 0, 0] || kind == 2 && p == [d[0] / 2 + 1, 0, 0]);
        let mut rows = vec![row(1., &[]); n];
        let mut known = vec![0.; n];
        for z in 0..d[2] {
            for y in 0..d[1] {
                for x in 0..d[0] {
                    let p = [x, y, z];
                    let i = index(p);
                    if obstacle(p) || pin(p) {
                        continue;
                    }
                    known[i] = ((i * 7) % 19) as f64 / 8.;
                    let mut edges = Vec::new();
                    let mut degree = 0;
                    for axis in 0..3 {
                        for sign in [-1isize, 1] {
                            let q = p[axis] as isize + sign;
                            if q < 0 || q >= d[axis] as isize {
                                continue;
                            }
                            let mut qpoint = p;
                            qpoint[axis] = q as usize;
                            if obstacle(qpoint) {
                                continue;
                            }
                            degree += 1;
                            if !pin(qpoint) {
                                edges.push(index(qpoint) as u32);
                            }
                        }
                    }
                    rows[i] = row(if kind == 0 { 6. } else { degree as f32 }, &edges);
                }
            }
        }
        for i in 0..n {
            rows[i].rhs = (rows[i].diagonal as f64 * known[i]
                - rows[i]
                    .neighbors
                    .iter()
                    .filter(|&&j| j != MISSING)
                    .map(|&j| known[j as usize])
                    .sum::<f64>()) as f32;
        }
        (rows, known)
    }
    // Independent dense f64 CG oracle, without Jacobi preconditioning or any
    // production reduction/multiply code. Used only for tiny hardware fixtures.
    fn oracle(rows: &[PressureRow]) -> Vec<f64> {
        let n = rows.len();
        let mut a = vec![vec![0.; n]; n];
        for (i, row) in rows.iter().enumerate() {
            a[i][i] = row.diagonal as f64;
            for &j in &row.neighbors {
                if j != MISSING {
                    a[i][j as usize] = -1.;
                }
            }
        }
        let mul = |x: &[f64]| {
            a.iter()
                .map(|row| row.iter().zip(x).map(|(a, x)| a * x).sum::<f64>())
                .collect::<Vec<_>>()
        };
        let dot = |a: &[f64], b: &[f64]| a.iter().zip(b).map(|(a, b)| a * b).sum::<f64>();
        let mut x = vec![0.; n];
        let mut r = rows.iter().map(|r| r.rhs as f64).collect::<Vec<_>>();
        let mut p = r.clone();
        let mut rr = dot(&r, &r);
        for _ in 0..n * 4 {
            if rr.sqrt() < 1e-11 {
                break;
            }
            let ap = mul(&p);
            let alpha = rr / dot(&p, &ap);
            for i in 0..n {
                x[i] += alpha * p[i];
                r[i] -= alpha * ap[i];
            }
            let next = dot(&r, &r);
            for i in 0..n {
                p[i] = r[i] + next / rr * p[i];
            }
            rr = next;
        }
        x
    }
    #[test]
    fn manufactured_matrices_have_known_solutions_and_explicit_gauges() {
        for kind in 0..3 {
            let (rows, known) = fixture([6, 4, 3], kind);
            validate(&rows, PressureConfig::default()).unwrap();
            let x = oracle(&rows);
            assert!(x.iter().zip(known).all(|(x, k)| (x - k).abs() < 1e-8));
        }
    }
    #[test]
    #[ignore = "explicit actual-GPU manufactured pressure parity; no physical-fluid promotion or performance claim"]
    fn gpu_manufactured_pressure_matches_dense_f64_oracle() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .expect("GPU adapter");
        assert_ne!(
            adapter.get_info().device_type,
            wgpu::DeviceType::Cpu,
            "hardware parity requires an actual GPU"
        );
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("pressure diagnostic"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: Default::default(),
            },
            None,
        ))
        .unwrap();
        for kind in 0..3 {
            let (rows, known) = fixture([10, 8, 5], kind);
            let config = PressureConfig {
                max_iterations: 256,
                relative_tolerance: 2e-6,
                ..Default::default()
            };
            let gpu = PhysicalPressureGpu::new(&device, &rows, config).unwrap();
            let mut encoder = device.create_command_encoder(&Default::default());
            gpu.encode(&mut encoder);
            queue.submit(Some(encoder.finish()));
            let result = gpu.readback_diagnostic(&device, &queue).unwrap();
            let cpu = oracle(&rows);
            let residual = rows
                .iter()
                .enumerate()
                .map(|(i, row)| {
                    let ax = row.diagonal as f64 * result.solution[i] as f64
                        - row
                            .neighbors
                            .iter()
                            .filter(|&&j| j != MISSING)
                            .map(|&j| result.solution[j as usize] as f64)
                            .sum::<f64>();
                    (row.rhs as f64 - ax).powi(2)
                })
                .sum::<f64>()
                .sqrt();
            let rhs = rows
                .iter()
                .map(|r| (r.rhs as f64).powi(2))
                .sum::<f64>()
                .sqrt();
            let max_error = result
                .solution
                .iter()
                .zip(&cpu)
                .map(|(x, y)| (*x as f64 - y).abs())
                .fold(0., f64::max);
            eprintln!("pressure_gpu adapter={:?} fixture={kind} rows={} iterations={} status={} reported_true_residual={} independent_residual={residual} relative_residual={} max_oracle_error={max_error} budget_bytes={}",adapter.get_info(),rows.len(),result.state.iterations,result.state.status,result.state.residual_squared.sqrt(),residual/rhs,gpu.allocated_and_diagnostic_budget_bytes);
            assert_eq!(result.state.status, 1);
            assert!(
                residual
                    <= (config.relative_tolerance as f64 * rhs)
                        .max(config.absolute_tolerance as f64)
            );
            assert!(max_error < 2e-4);
            assert!(result
                .solution
                .iter()
                .zip(&known)
                .all(|(x, k)| (*x as f64 - k).abs() < 2e-4));
        }
    }
    #[test]
    #[ignore = "optional quiet-machine128K pressure microbenchmark; not physical-fluid or game performance"]
    fn gpu_pressure_128k_component_microbenchmark() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .unwrap();
        assert_ne!(adapter.get_info().device_type, wgpu::DeviceType::Cpu);
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("pressure128K diagnostic"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: Default::default(),
            },
            None,
        ))
        .unwrap();
        let (rows, _) = fixture([64, 64, 32], 0);
        let gpu = PhysicalPressureGpu::new(
            &device,
            &rows,
            PressureConfig {
                max_iterations: 256,
                ..Default::default()
            },
        )
        .unwrap();
        for run in 0..2 {
            let start = std::time::Instant::now();
            let mut encoder = device.create_command_encoder(&Default::default());
            gpu.encode(&mut encoder);
            queue.submit(Some(encoder.finish()));
            let result = gpu.readback_diagnostic(&device, &queue).unwrap();
            eprintln!("pressure128k run={run} adapter={:?} command_plus_gpu_plus_final_readback_ms={} iterations={} status={} relative_true_residual={} budget_bytes={} scope=component_only_not_game_fps_no_per_iteration_readbacks",adapter.get_info(),start.elapsed().as_secs_f64()*1000.,result.state.iterations,result.state.status,(result.state.residual_squared/result.state.rhs_squared).sqrt(),gpu.allocated_and_diagnostic_budget_bytes);
            assert!(result.state.residual_squared.is_finite());
        }
    }
}
