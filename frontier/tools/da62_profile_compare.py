"""Compare the model studio's 1600x1000 orthographic DA62 captures to Diamond's 711x400 drawing.

Use front at distance 14.6 m and side at 11.2 m with the default target and gear down.
Run with DRAWING.png CAPTURE_DIRECTORY OUTPUT_PREFIX.
"""
import json
import math
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont

drawing, frames, dest = map(Path, sys.argv[1:])
paper = Image.open(drawing).convert("RGB")
if paper.size != (711, 400):
    raise ValueError("Use Diamond's original 711x400 DA62 technical drawing.")
font = ImageFont.truetype("DejaVuSans.ttf", 18)
ground_y = -1.359375
camera_y = 0.16
camera_z = -0.85
span_scale = 513 / 14.55
length_scale = 420 / 9.19
# Origin is fixed to the front centerline or the side-view nose at z=3.25 m.
views = [
    ("front", 14.6, span_scale, 104 / 2.82, 355.5, 156, (87, 48, 624, 162)),
    ("side", 11.2, length_scale, 136 / 2.82, 613 - (3.25-camera_z)*length_scale, 324, (190,183,620,330)),
]


def intervals(mask, x, top, bottom):
    return np.flatnonzero(mask[top:bottom+1, x]) + top


def errors(view, projected, ref):
    # Static exterior at the same stations in the drawing and the model. Exclude
    # prop discs/blades, wheels and struts from the side upper/lower envelopes.
    if view == "side":
        bands = [(range(204, 518), 186, 289, "top"), (range(538, 609), 220, 289, "top"),
                 (range(204, 443), 186, 289, "bottom"), (range(473, 521), 186, 289, "bottom"),
                 (range(557, 609), 186, 289, "bottom")]
    else:
        bands = [(xs, 84, 118, direction) for xs in (range(102, 254), range(457, 610))
                 for direction in ("top", "bottom")]
    delta = []
    locations = []
    for xs, top, bottom, direction in bands:
        i = 0 if direction == "top" else -1
        for x in xs:
            a, b = intervals(ref,x,top,bottom), intervals(projected,x,top,bottom)
            if len(a) and len(b):
                delta.append(abs(int(a[i])-int(b[i])))
                locations.append((x, direction))
    return dict(samples=len(delta),mean_px=round(float(np.mean(delta)),2),
                p95_px=float(np.percentile(delta,95)),max_px=int(max(delta)),
                worst_station=locations[int(np.argmax(delta))])


ref = np.asarray(paper.convert("L")) < 210
report = {}
for calibrated in (False, True):
    panels = []
    rendered_panels = []
    mode = "dimensions" if calibrated else "original"
    report[mode] = {}
    for view,distance,sx,sy_height,cx,ground,crop in views:
        sy = sy_height if calibrated else sx
        cy = ground - (camera_y-ground_y)*sy
        ppm = 1000/(2*distance*math.tan(math.radians(43/2)))
        rgb = np.asarray(Image.open(frames/f"{view}.png").convert("RGB")).astype(int)
        if rgb.shape[:2] != (1000,1600):
            raise ValueError("Use 1600x1000 captures.")
        mask = Image.fromarray((np.max(abs(rgb-rgb[:,:1]),axis=2)>4).astype("uint8")*255)
        if view == "side":
            mask = mask.transpose(Image.Transpose.FLIP_LEFT_RIGHT)

        def project(img, box, scale):
            size = ((box[2]-box[0])*scale,(box[3]-box[1])*scale)
            transform = (ppm/(scale*sx),0,800+(box[0]-cx)*ppm/sx,
                         0,ppm/(scale*sy),500+(box[1]-cy)*ppm/sy)
            return img.transform(size,Image.Transform.AFFINE,transform,Image.Resampling.NEAREST)

        projected = np.asarray(project(mask,(0,0,*paper.size),1))>0
        report[mode][view] = errors(view, projected, ref)
        scale = 4
        silhouette = project(mask,crop,scale)
        base = paper.crop(crop).resize(silhouette.size,Image.Resampling.BICUBIC)
        fill = Image.composite(Image.new("RGB",base.size,"#007fbd"),base,silhouette)
        base = Image.blend(base,fill,0.16)
        edge = np.asarray(silhouette)>np.asarray(silhouette.filter(ImageFilter.MinFilter(5)))
        base.paste("#007cb4",mask=Image.fromarray(edge))
        if view == "front":
            # An isolated front view includes each entire rotor. Threefold blade
            # symmetry puts its projected area centroid at the shaft center.
            pen = ImageDraw.Draw(base)
            for name in ("left-propeller", "right-propeller"):
                path = frames/f"{name}.png"
                if not path.exists():
                    continue
                rotor = np.asarray(Image.open(path).convert("RGB")).astype(int)
                ys,xs = np.where(np.max(abs(rotor-rotor[:,:1]),axis=2)>12)
                col,row = xs.mean(),ys.mean()
                radius = float(np.hypot(xs-col,ys-row).max())/ppm
                x=(cx+(col-800)/ppm*sx-crop[0])*scale
                y=(cy+(row-500)/ppm*sy-crop[1])*scale
                rx,ry=radius*sx*scale,radius*sy*scale
                for angle in range(0,360,18):
                    pen.arc((x-rx,y-ry,x+rx,y+ry),angle,angle+9,fill="#bb5915",width=3)
        colors = Image.fromarray(rgb.astype("uint8"))
        if view=="side":
            colors = colors.transpose(Image.Transpose.FLIP_LEFT_RIGHT)
        paint = project(colors,crop,scale)
        paper_crop = paper.crop(crop).resize(silhouette.size,Image.Resampling.BICUBIC)
        tinted = Image.blend(Image.new("RGB",paint.size,"white"),paint,.50)
        ink = np.asarray(paper_crop).astype(float)/255
        ghost = Image.composite(Image.fromarray((np.asarray(tinted)*ink).astype("uint8")),paper_crop,silhouette)
        ghost.thumbnail((1450,512),Image.Resampling.LANCZOS)
        rendered_panels.append((view,ghost))
        base.thumbnail((1450,512),Image.Resampling.LANCZOS)
        panels.append((view,base))
    out = Image.new("RGB",(1490,sum(im.height+44 for _,im in panels)+98),"white")
    draw = ImageDraw.Draw(out)
    title = "Horizontal and vertical scales from printed dimensions" if calibrated else "Uniform scale from printed wingspan or length"
    draw.text((20,8),"Diamond DA62   |   "+title,fill="black",font=font)
    draw.text((20,43),"Gray: manufacturer drawing     Blue: rendered contour     Dashed orange, where available: measured blade sweep     Datum: wheel contact",fill="#285669",font=font)
    y=98
    for view,im in panels:
        draw.text((20,y),view.upper(),fill="black",font=font)
        out.paste(im,(20,y+24))
        y+=im.height+44
    out.save(dest.parent/f"{dest.name}-{mode}.png")
    if calibrated:
        draw.rectangle((0,36,1490,85),fill="white")
        draw.text((20,43),"Actual studio render at 50% opacity. Diamond’s original linework is visible through the model.",fill="#285669",font=font)
        y=98
        for _,im in rendered_panels:
            out.paste(im,(20,y+24))
            y+=im.height+44
        out.save(dest.parent/f"{dest.name}-rendered.png")
(dest.parent/f"{dest.name}-errors.json").write_text(json.dumps(report,indent=2)+"\n")
