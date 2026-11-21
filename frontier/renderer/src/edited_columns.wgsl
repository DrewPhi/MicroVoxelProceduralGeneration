fn edit_owns_column(mask: texture_2d<u32>, cell: vec2<u32>) -> bool {
    let dims = textureDimensions(mask);
    if dims.y == 1u { return false; }
    let capacity = dims.x * (dims.y - 1u);
    let bounds = textureLoad(mask, vec2<i32>(0, i32(dims.y-1u)), 0);
    if any(cell - bounds.xy > bounds.zw) { return false; }
    let key = cell >> vec2(3u);
    let h = key.x*0x9e3779b1u ^ key.y*0x85ebca77u;
    var bucket = (h^(h>>16u)) & (capacity-1u);
    let bit = (cell.x&7u) + (cell.y&7u)*8u;
    for (var probe=0u; probe<capacity; probe++) {
        let word = textureLoad(mask,vec2<i32>(i32(bucket%dims.x),i32(bucket/dims.x)),0);
        if (word.z | word.w)==0u {return false;}
        if all(key==word.xy) {return (select(word.z, word.w, bit>=32u) & (1u << (bit&31u))) != 0u;}
        bucket = (bucket+1u) & (capacity-1u);
    }
    return false;
}
