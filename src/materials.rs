//! Compact cell values and shared definitions. Property maps start EMPTY.
use std::collections::BTreeMap;

pub const MATERIAL_SCHEMA: u32 = 2;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MaterialId(pub u16);
impl MaterialId {
    pub const AIR: Self = Self(0);
    pub const ROCK: Self = Self(1);
    pub const SOIL: Self = Self(2);
    pub const SAND: Self = Self(3);
    pub const GRAVEL: Self = Self(4);
    pub const WATER: Self = Self(5);
    pub const SNOW: Self = Self(6);
    pub const GRASS: Self = Self(7);
    pub const SANDSTONE: Self = Self(8);
    pub const LIMESTONE: Self = Self(9);
    pub const BASALT: Self = Self(10);
    pub const MUD: Self = Self(11);
    pub const SILT: Self = Self(12);
    pub fn is_bedrock(self) -> bool {
        matches!(
            self,
            Self::ROCK | Self::SANDSTONE | Self::LIMESTONE | Self::BASALT
        )
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Appearance(pub u16);

/// Four bytes. Moving a value preserves its appearance; there are no per-face colors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Voxel {
    pub material: MaterialId,
    pub appearance: Appearance,
}
impl Voxel {
    pub const AIR: Self = Self {
        material: MaterialId::AIR,
        appearance: Appearance(0),
    };
    pub const fn new(material: MaterialId, appearance: Appearance) -> Self {
        Self {
            material,
            appearance,
        }
    }
    pub fn is_air(self) -> bool {
        self.material == MaterialId::AIR
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterialKind {
    Air,
    Solid,
    Fluid,
}

/// Extension values, not built-in physics: callers define property names/units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PropertyValue {
    Flag(bool),
    Integer(i64),
    Fixed { raw: i64, fractional_bits: u8 },
}
#[derive(Clone, Debug)]
pub struct MaterialDefinition {
    pub id: MaterialId,
    pub name: String,
    pub kind: MaterialKind,
    pub properties: BTreeMap<String, PropertyValue>,
    pub palette: [[u8; 3]; 2],
}
#[derive(Clone, Debug)]
pub struct MaterialRegistry {
    definitions: Vec<MaterialDefinition>,
}
impl Default for MaterialRegistry {
    fn default() -> Self {
        let entries = [
            ("air", MaterialKind::Air),
            ("rock", MaterialKind::Solid),
            ("soil", MaterialKind::Solid),
            ("sand", MaterialKind::Solid),
            ("gravel", MaterialKind::Solid),
            ("water", MaterialKind::Fluid),
            ("snow", MaterialKind::Solid),
            ("grass", MaterialKind::Solid),
            ("sandstone", MaterialKind::Solid),
            ("limestone", MaterialKind::Solid),
            ("basalt", MaterialKind::Solid),
            ("mud", MaterialKind::Solid),
            ("silt", MaterialKind::Solid),
        ];
        Self {
            definitions: entries
                .iter()
                .enumerate()
                .map(|(i, (name, kind))| MaterialDefinition {
                    id: MaterialId(i as u16),
                    name: (*name).into(),
                    kind: *kind,
                    properties: BTreeMap::new(),
                    palette: crate::painting::default_palette(MaterialId(i as u16)),
                })
                .collect(),
        }
    }
}
impl MaterialRegistry {
    pub fn fingerprint(&self) -> u64 {
        let mut hash = 14695981039346656037u64;
        for def in &self.definitions {
            let data = format!(
                "{}:{}:{:?}:{:?};",
                def.id.0,
                def.name,
                def.kind,
                (&def.properties, def.palette)
            );
            for byte in data.bytes() {
                hash ^= byte as u64;
                hash = hash.wrapping_mul(1099511628211);
            }
        }
        hash
    }
    pub fn get(&self, id: MaterialId) -> Option<&MaterialDefinition> {
        self.definitions.get(id.0 as usize)
    }
    pub fn get_mut(&mut self, id: MaterialId) -> Option<&mut MaterialDefinition> {
        self.definitions.get_mut(id.0 as usize)
    }
    pub fn definitions(&self) -> &[MaterialDefinition] {
        &self.definitions
    }
    pub fn register(&mut self, name: String, kind: MaterialKind) -> Option<MaterialId> {
        let id = MaterialId(u16::try_from(self.definitions.len()).ok()?);
        self.definitions.push(MaterialDefinition {
            id,
            name,
            kind,
            properties: BTreeMap::new(),
            palette: crate::painting::default_palette(id),
        });
        Some(id)
    }
}
