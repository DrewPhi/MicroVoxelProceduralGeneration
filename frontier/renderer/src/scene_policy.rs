//! Renderer capability policy, independent of a scene's terrain/content.
//! Keep historical comparison recipes frozen; current playable scenes share the
//! same quality, filtering, HDR and shadow paths.
use frontier_world::{FbmRecipe, World};
pub(crate) fn polished(recipe: FbmRecipe) -> bool {
    matches!(recipe, FbmRecipe::Alpine | FbmRecipe::Desert)
}
pub(crate) fn polished_world(world: &World) -> bool {
    world
        .fbm_terrain()
        .is_some_and(|t| polished(t.params().recipe))
}
