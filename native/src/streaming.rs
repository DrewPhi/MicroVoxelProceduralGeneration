//! Bounded CPU mesh jobs. Meshes are immutable results, consumed on the GPU
//! thread. Queue backpressure prevents travel from spawning unbounded work.
use microvoxel::{
    generation::ProceduralTerrain,
    render::{
        lod::MeshKey,
        terrain::{Sampler, TerrainMesh},
    },
    World,
};
use std::{
    sync::{
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
pub struct Completed {
    pub key: MeshKey,
    pub mesh: Result<TerrainMesh, String>,
    pub elapsed: Duration,
}
pub struct Workers {
    pub sender: SyncSender<MeshKey>,
    pub results: Receiver<Completed>,
    pub count: usize,
}
impl Workers {
    pub fn new(world: Arc<World<ProceduralTerrain>>) -> Self {
        let count =
            thread::available_parallelism().map_or(2, |n| n.get().saturating_sub(1).clamp(1, 4));
        let (sender, requests) = mpsc::sync_channel::<MeshKey>(count * 2);
        let (results_tx, results) = mpsc::sync_channel(count);
        let requests = Arc::new(Mutex::new(requests));
        for id in 0..count {
            let requests = Arc::clone(&requests);
            let tx = results_tx.clone();
            let world = Arc::clone(&world);
            thread::Builder::new()
                .name(format!("voxel-mesh-{id}"))
                .spawn(move || {
                    let mut sampler = Sampler::new(world);
                    loop {
                        let request = requests.lock().unwrap().recv();
                        let Ok(key) = request else {
                            break;
                        };
                        let start = Instant::now();
                        let mesh = sampler.mesh(key);
                        if tx
                            .send(Completed {
                                key,
                                mesh,
                                elapsed: start.elapsed(),
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                })
                .expect("mesh worker");
        }
        Self {
            sender,
            results,
            count,
        }
    }
    pub fn submit(&self, key: MeshKey) -> bool {
        match self.sender.try_send(key) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => false,
            Err(TrySendError::Disconnected(_)) => false,
        }
    }
}
