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

use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tile {
    Grow(char, u8),
    Turn(bool),
    Continue(char),
    Branch(Vec<Tile>),
}
impl Tile {
    fn write(&self, out: &mut String) {
        match self {
            Self::Grow(action, count) => {
                out.push(*action);
                if *count != 1 {
                    out.push_str(&count.to_string());
                }
            }
            Self::Turn(right) => out.push(if *right { '+' } else { '-' }),
            Self::Continue(tip) => out.push(*tip),
            Self::Branch(body) => {
                out.push('[');
                for tile in body {
                    tile.write(out);
                }
                out.push(']');
            }
        }
    }
    pub fn code(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeedProgram {
    pub start: Vec<char>,
    pub rules: BTreeMap<char, Vec<Tile>>,
}
impl Default for SeedProgram {
    fn default() -> Self {
        Self::parse("T", "T = V3 L [B] + T\nB = F2 U L B").expect("starter seed is valid")
    }
}
impl SeedProgram {
    pub fn available_name(&self) -> Option<char> {
        ('A'..='Z').find(|c| !"FUWVL".contains(*c) && !self.rules.contains_key(c))
    }
    pub fn source(&self) -> (String, String) {
        let start = self.start.iter().collect();
        let rules = self
            .rules
            .iter()
            .map(|(name, tiles)| {
                format!(
                    "{name} = {}",
                    tiles.iter().map(Tile::code).collect::<Vec<_>>().join(" ")
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        (start, rules)
    }
    pub fn parse(start: &str, source: &str) -> Result<Self, &'static str> {
        if start.len() > 64 || source.len() > 8192 {
            return Err("Program is too long");
        }
        fn body(
            chars: &mut std::iter::Peekable<impl Iterator<Item = char>>,
            depth: usize,
        ) -> Result<Vec<Tile>, &'static str> {
            if depth > 8 {
                return Err("Branches can nest eight levels deep");
            }
            let mut tiles = Vec::new();
            while let Some(c) = chars.next() {
                if c == ']' {
                    return if depth > 0 {
                        Ok(tiles)
                    } else {
                        Err("Unexpected closing bracket")
                    };
                }
                let tile = match c {
                    '[' => Tile::Branch(body(chars, depth + 1)?),
                    '+' | '-' => Tile::Turn(c == '+'),
                    'F' | 'U' | 'W' | 'V' | 'L' => {
                        let mut count = 0;
                        let mut has_digits = false;
                        while let Some(digit) = chars.peek().and_then(|c| c.to_digit(10)) {
                            chars.next();
                            has_digits = true;
                            count = count * 10 + digit;
                            if count > 64 {
                                return Err("Repeat counts must be between 1 and 64");
                            }
                        }
                        if has_digits && count == 0 {
                            return Err("Repeat counts must be between 1 and 64");
                        }
                        Tile::Grow(c, count.max(1) as u8)
                    }
                    'A'..='Z' => Tile::Continue(c),
                    _ => return Err("Unknown growth symbol"),
                };
                tiles.push(tile);
            }
            if depth == 0 {
                Ok(tiles)
            } else {
                Err("Unclosed branch")
            }
        }
        let mut rules = BTreeMap::new();
        for line in source.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let (name, production) = line
                .split_once('=')
                .ok_or("Use one rule per line, such as T = V3 T")?;
            let name = name.trim();
            let c = name
                .chars()
                .next()
                .filter(|c| c.is_ascii_uppercase() && !"FUWVL".contains(*c))
                .filter(|_| name.len() == 1)
                .ok_or("Use one unreserved capital letter for each tip")?;
            let mut chars = production.chars().filter(|c| !c.is_whitespace()).peekable();
            if rules.insert(c, body(&mut chars, 0)?).is_some() {
                return Err("Duplicate tip rule");
            }
        }
        fn valid(tiles: &[Tile], rules: &BTreeMap<char, Vec<Tile>>) -> bool {
            tiles.iter().all(|tile| match tile {
                Tile::Continue(c) => rules.contains_key(c),
                Tile::Branch(tiles) => valid(tiles, rules),
                _ => true,
            })
        }
        let start: Vec<_> = start.chars().filter(|c| !c.is_whitespace()).collect();
        if start.is_empty() || !start.iter().all(|c| rules.contains_key(c)) {
            return Err("Every starting tip needs a rule");
        }
        if !rules.values().all(|tiles| valid(tiles, &rules)) {
            return Err("A continuation calls an undefined tip");
        }
        Ok(Self { start, rules })
    }
    pub(crate) fn tips(&self, origin: Tip) -> Vec<Tip> {
        self.start
            .iter()
            .map(|&symbol| Tip {
                segment: symbol as u32,
                ..origin
            })
            .collect()
    }
    pub(crate) fn has_rule(&self, segment: u32) -> bool {
        char::from_u32(segment).is_some_and(|c| self.rules.contains_key(&c))
    }
    pub(crate) fn expand(&self, tip: Tip) -> (Vec<Step>, Vec<Tip>) {
        fn expand(tiles: &[Tile], mut tip: Tip, steps: &mut Vec<Step>, next: &mut Vec<Tip>) {
            for tile in tiles {
                if steps.len() >= 1024 || next.len() >= MAX_TIPS {
                    return;
                }
                match tile {
                    Tile::Grow('L', count) => {
                        for sign in [-1, 1] {
                            let mut parent = tip.cell;
                            for i in 0..*count {
                                let cell = add(parent, -tip.dz * sign, 0, tip.dx * sign);
                                steps.push(Step {
                                    cell,
                                    parent,
                                    tissue: Tissue::Leaf,
                                });
                                if i + 1 < *count {
                                    for tuft in [add(cell, -tip.dx, 0, -tip.dz), add(cell, 0, 1, 0)]
                                    {
                                        steps.push(Step {
                                            cell: tuft,
                                            parent: cell,
                                            tissue: Tissue::Leaf,
                                        });
                                    }
                                }
                                parent = cell;
                            }
                        }
                    }
                    Tile::Grow(action, count) => {
                        let delta = if matches!(action, 'U' | 'V') {
                            (0, 1, 0)
                        } else {
                            (tip.dx, 0, tip.dz)
                        };
                        let tissue = if matches!(action, 'W' | 'V') {
                            Tissue::AdaptiveStem
                        } else {
                            Tissue::Stem
                        };
                        for _ in 0..*count {
                            let cell = add(tip.cell, delta.0, delta.1, delta.2);
                            steps.push(Step {
                                cell,
                                parent: tip.cell,
                                tissue,
                            });
                            tip.cell = cell;
                        }
                    }
                    Tile::Turn(true) => (tip.dx, tip.dz) = (-tip.dz, tip.dx),
                    Tile::Turn(false) => (tip.dx, tip.dz) = (tip.dz, -tip.dx),
                    Tile::Continue(symbol) => next.push(Tip {
                        segment: *symbol as u32,
                        ..tip
                    }),
                    Tile::Branch(tiles) => expand(tiles, tip, steps, next),
                }
            }
        }
        let mut steps = Vec::new();
        let mut next = Vec::new();
        if let Some(tiles) = char::from_u32(tip.segment).and_then(|s| self.rules.get(&s)) {
            expand(tiles, tip, &mut steps, &mut next);
        }
        (steps, next)
    }
    /// Idealized, bounded preview of the same tip instructions the garden runs.
    pub fn preview(&self, rounds: usize) -> (BTreeMap<Cell, Tissue>, Vec<char>) {
        let base = Cell { x: 0, y: 0, z: 0 };
        let mut tissue = BTreeMap::from([(base, Tissue::Stem)]);
        let mut history = Vec::new();
        let mut tips = self.tips(Tip {
            cell: base,
            shoot: true,
            dx: 1,
            dz: 0,
            segment: 0,
        });
        let mut ran = Vec::new();
        for _ in 0..rounds.min(24) {
            ran.clear();
            let mut next = Vec::new();
            for tip in tips.into_iter().take(MAX_TIPS) {
                if !tissue.get(&tip.cell).is_some_and(|t| t.is_stem()) {
                    continue;
                }
                if let Some(symbol) = char::from_u32(tip.segment) {
                    ran.push(symbol);
                }
                let (steps, successors) = self.expand(tip);
                for step in steps {
                    if tissue.len() >= 16_000 {
                        break;
                    }
                    if tissue.contains_key(&step.parent) && !tissue.contains_key(&step.cell) {
                        tissue.insert(step.cell, step.tissue);
                        history.push(step);
                    }
                }
                next.extend(successors.into_iter().take(MAX_TIPS - next.len()));
            }
            let mut reserved = BTreeSet::new();
            for step in reinforcement(&history, &tissue, &mut reserved) {
                if tissue.len() >= 16_000 {
                    break;
                }
                tissue.insert(step.cell, step.tissue);
                history.push(step);
            }
            tips = next;
        }
        (tissue, ran)
    }
}

// Accumulate supported tissue weight along the placement parents. Adaptive
// stems add a neighboring load-bearing voxel when the local section is loaded.
pub(crate) fn reinforcement(
    history: &[Step],
    tissue: &BTreeMap<Cell, Tissue>,
    reserved: &mut BTreeSet<Cell>,
) -> Vec<Step> {
    let mut load = BTreeMap::<Cell, u32>::new();
    for s in history
        .iter()
        .rev()
        .filter(|s| tissue.contains_key(&s.cell))
    {
        let weight = if s.tissue == Tissue::Leaf { 1 } else { 3 };
        let carried = load.entry(s.cell).or_default();
        *carried += weight;
        let carried = *carried;
        *load.entry(s.parent).or_default() += carried;
    }
    let mut result = Vec::new();
    for (&cell, &kind) in tissue {
        if kind != Tissue::AdaptiveStem || result.len() >= 64 {
            continue;
        }
        let neighbors = [
            add(cell, 1, 0, 0),
            add(cell, -1, 0, 0),
            add(cell, 0, 0, 1),
            add(cell, 0, 0, -1),
        ];
        let support = neighbors
            .iter()
            .filter(|p| tissue.get(p).is_some_and(|t| t.is_stem()))
            .count() as u32;
        if load.get(&cell).copied().unwrap_or(0) <= 66 * (support + 1) {
            continue;
        }
        if let Some(next) = neighbors
            .into_iter()
            .find(|p| !tissue.contains_key(p) && reserved.insert(*p))
        {
            result.push(Step {
                cell: next,
                parent: cell,
                tissue: Tissue::Reinforcement,
            });
        }
    }
    result
}
