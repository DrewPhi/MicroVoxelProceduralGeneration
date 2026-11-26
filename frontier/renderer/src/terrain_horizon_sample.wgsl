fn terrain_horizon_light(sun:vec3f,receiver_y:f32,h:vec4f)->vec2f {
    if sun.y<=0. {return vec2f(1.,h.y);}
    let slope=sun.y/max(.0001,length(sun.xz));
    let blocker=exp2(h.z);
    let receiver=receiver_y-h.w;
    let horizon=h.x-clamp(receiver/blocker,-.38,.38);
    let penumbra=.012+min(.068,12./blocker);
    return vec2f(mix(.11,1.,smoothstep(-penumbra,penumbra,slope-horizon)),h.y);
}
