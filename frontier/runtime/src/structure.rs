//! Reduced structural model for articulated voxel objects. Wrenches are in
//! object axes and moments are about the loaded node's center of mass.
use glam::Vec3;

#[derive(Clone, Copy, Debug, Default)]
pub struct Load {
    pub force: Vec3,
    pub moment: Vec3,
}

impl Load {
    pub fn apply(&mut self, force: Vec3, arm: Vec3) {
        self.force += force;
        self.moment += arm.cross(force);
    }
}

/// Calibrated strength of a member in newtons and newton metres. A joint's
/// members work in parallel. Lower stiffness from yielding transfers load.
#[derive(Clone, Copy, Debug)]
pub struct Member {
    pub force: Vec3,
    pub moment: Vec3,
    pub stiffness: f32,
    pub toughness: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Joint {
    pub parent: usize,
    pub anchor: Vec3,
    pub axes: glam::Mat3,
    pub members: [Member; 3],
}

#[derive(Clone, Copy, Debug)]
pub struct Node {
    pub center: Vec3,
    pub mass: f32,
    pub joint: Option<Joint>,
}

#[derive(Clone, Copy, Default)]
struct State {
    work: [f32; 3],
    severed: bool,
}

/// Nodes are structural assemblies. The solve order places parents before their children.
/// `solve` uses caller-owned input and internal scratch space without allocating.
#[derive(Clone)]
pub struct Structure<const N: usize> {
    states: [State; N],
    reactions: [Load; N],
    pub peak_utilization: f32,
}

impl<const N: usize> Default for Structure<N> {
    fn default() -> Self {
        assert!(N <= 64);
        Self {
            states: [State::default(); N],
            reactions: [Load::default(); N],
            peak_utilization: 0.,
        }
    }
}

impl<const N: usize> Structure<N> {
    pub fn reaction(&self, node: usize) -> Load {
        self.reactions[node]
    }

    /// Return masks of disconnected assemblies. Mask 0 in the result is the
    /// surviving root component, followed by the newly separated components.
    /// A false return means no connectivity changed during this step.
    pub fn solve(
        &mut self,
        nodes: &[Node; N],
        active: u64,
        loads: &[Load; N],
        dt: f32,
        detached: &mut Vec<u64>,
    ) -> bool {
        self.solve_in_order(nodes, active, loads, dt, detached, 1..N)
    }

    /// Solve using a parent-first traversal. Node zero remains the root.
    pub fn solve_in_order(
        &mut self,
        nodes: &[Node; N],
        active: u64,
        loads: &[Load; N],
        dt: f32,
        detached: &mut Vec<u64>,
        order: impl DoubleEndedIterator<Item = usize> + Clone,
    ) -> bool {
        self.reactions = *loads;
        self.peak_utilization = 0.;
        let mut changed = false;
        for child in order.clone().rev() {
            if active & (1 << child) == 0 {
                continue;
            }
            let node = nodes[child];
            let joint = node.joint.expect("non-root structure node has a joint");
            let state = &mut self.states[child];
            if state.severed || active & (1 << joint.parent) == 0 {
                continue;
            }
            let load = self.reactions[child];
            let moment = load.moment + (node.center - joint.anchor).cross(load.force);
            let frame = joint.axes.transpose();
            let force = frame * load.force;
            let bend = frame * moment;

            // Yielding softens a member, so surviving members progressively
            // take its share. Toughness is the normalized failure work.
            let weights: [f32; 3] = std::array::from_fn(|i| {
                let m = &joint.members[i];
                let life = (1. - state.work[i] / m.toughness).max(0.);
                if life == 0. {
                    0.
                } else {
                    m.stiffness * (0.25 + 0.75 * life)
                }
            });
            let total: f32 = weights.iter().sum();
            for (i, share) in weights.into_iter().enumerate() {
                if share == 0. {
                    continue;
                }
                let m = joint.members[i];
                let utilization = ((force / m.force).length_squared()
                    + (bend / m.moment).length_squared())
                .sqrt()
                    * (share / total);
                self.peak_utilization = self.peak_utilization.max(utilization);
                let plastic = (utilization - 1.).max(0.);
                state.work[i] = (state.work[i] + plastic * (1. + plastic) * dt).min(m.toughness);
            }
            if state
                .work
                .iter()
                .zip(&joint.members)
                .all(|(work, m)| *work >= m.toughness)
            {
                state.severed = true;
                changed = true;
                continue;
            }

            let parent = &mut self.reactions[joint.parent];
            parent.apply(load.force, joint.anchor - nodes[joint.parent].center);
            parent.moment += moment;
        }
        if !changed {
            return false;
        }
        let mut component = [0; N];
        detached.clear();
        detached.push(active & 1);
        for i in order {
            if active & (1 << i) == 0 {
                continue;
            }
            let parent = nodes[i].joint.unwrap().parent;
            component[i] = if self.states[i].severed || active & (1 << parent) == 0 {
                detached.push(0);
                detached.len() - 1
            } else {
                component[parent]
            };
            detached[component[i]] |= 1 << i;
        }
        detached.retain(|mask| *mask != 0);
        true
    }
}
