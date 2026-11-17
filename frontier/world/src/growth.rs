//! A small, data-driven recursive bud grammar. No finished plant is generated:
//! each active tip rewrites into a few cell instructions and successor tips.
use crate::{garden::Tissue, Cell};

pub const MAX_CELLS: usize = 65_536;
pub(crate) const MAX_STEPS: usize = MAX_CELLS * 4;
pub(crate) const MAX_TIPS: usize = 512;

/// These four integer rules are intended to become sliders in the seed editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GrowthRules {
    pub shoot_branch_every: u32,
    pub branch_fork_every: u32,
    pub branch_rise_every: u32,
    pub leaf_every: u32,
}
impl Default for GrowthRules {
    fn default() -> Self {
        Self {
            shoot_branch_every: 8,
            branch_fork_every: 12,
            branch_rise_every: 3,
            leaf_every: 2,
        }
    }
}
impl GrowthRules {
    pub fn valid(self) -> bool {
        [
            self.shoot_branch_every,
            self.branch_fork_every,
            self.branch_rise_every,
            self.leaf_every,
        ]
        .into_iter()
        .all(|v| (2..=64).contains(&v))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Step {
    pub cell: Cell,
    pub parent: Cell,
    pub tissue: Tissue,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Tip {
    pub cell: Cell,
    pub shoot: bool,
    pub dx: i64,
    pub dz: i64,
    pub segment: u32,
}
fn add(c: Cell, x: i64, y: i64, z: i64) -> Cell {
    Cell {
        x: c.x + x,
        y: c.y + y,
        z: c.z + z,
    }
}
impl Tip {
    pub fn valid(self) -> bool {
        matches!((self.dx, self.dz), (1, 0) | (-1, 0) | (0, 1) | (0, -1))
            && self.segment < 1_000_000
    }
    /// Shoot -> up + leaves + shoot + occasional branch.
    /// Branch -> out/up + leaves + branch + occasional perpendicular branch.
    /// Leaves are terminal. All tissue steps have a face-adjacent parent.
    pub fn rewrite(self, rules: GrowthRules) -> (Vec<Step>, Vec<Tip>) {
        let mut steps = Vec::new();
        let mut end = self.cell;
        let mut stem = |x, y, z| {
            let next = add(end, x, y, z);
            steps.push(Step {
                cell: next,
                parent: end,
                tissue: Tissue::Stem,
            });
            end = next;
        };
        let n = self.segment + 1;
        if self.shoot {
            stem(0, 1, 0);
        } else {
            stem(self.dx, 0, self.dz);
            if n % rules.branch_rise_every == 0 {
                stem(0, 1, 0);
            }
        }
        let fork = n % if self.shoot {
            rules.shoot_branch_every
        } else {
            rules.branch_fork_every
        } == 0;
        if n % rules.leaf_every == 0 && !fork {
            for sign in [-1, 1] {
                steps.push(Step {
                    cell: add(end, -self.dz * sign, 0, self.dx * sign),
                    parent: end,
                    tissue: Tissue::Leaf,
                });
            }
        }
        // A thickening shoot is still built from connected individual stem cells.
        if self.shoot && n > 12 {
            for (x, z) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                steps.push(Step {
                    cell: add(end, x, 0, z),
                    parent: end,
                    tissue: Tissue::Stem,
                });
            }
        }
        let mut next = vec![Tip {
            cell: end,
            segment: n,
            ..self
        }];
        if fork {
            next.push(Tip {
                cell: end,
                shoot: false,
                segment: 0,
                dx: self.dx,
                dz: self.dz,
            });
            // Rotate the next branch site; lateral branches fork perpendicular.
            if self.shoot {
                next[0].dx = -self.dz;
                next[0].dz = self.dx;
            } else {
                next[1].dx = -self.dz;
                next[1].dz = self.dx;
            }
        }
        (steps, next)
    }
}
