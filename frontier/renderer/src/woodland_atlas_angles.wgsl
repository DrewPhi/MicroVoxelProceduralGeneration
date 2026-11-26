const CROWN_YAW_STEP:f32=0.7853981633974483;
const CROWN_ELEVATION:array<f32,4>=array<f32,4>(-.38,.02,.53,1.11);
const CROWN_AZIMUTH_AXES:array<vec2f,8>=array<vec2f,8>(
    vec2f(1.,0.),vec2f(.70710678,.70710678),vec2f(0.,1.),vec2f(-.70710678,.70710678),
    vec2f(-1.,0.),vec2f(-.70710678,-.70710678),vec2f(0.,-1.),vec2f(.70710678,-.70710678));
const CROWN_ELEVATION_AXES:array<vec2f,4>=array<vec2f,4>(
    vec2f(.92866464,-.37092047),vec2f(.99980001,.01999867),
    vec2f(.86280707,.50553334),vec2f(.44466152,.89569869));
