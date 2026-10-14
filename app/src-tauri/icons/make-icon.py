from PIL import Image, ImageDraw
import math
S=1024; im=Image.new("RGBA",(S,S),(0,0,0,0)); d=ImageDraw.Draw(im)
# rounded square background
d.rounded_rectangle([40,40,S-40,S-40],radius=220,fill=(32,32,36,255))
c=S/2; r=300; w=78; col=(240,240,240,255)
def arc(a0,a1):
    d.arc([c-r,c-r,c+r,c+r],a0,a1,fill=col,width=w)
def head(angle_deg, direction):
    a=math.radians(angle_deg)
    px=c+r*math.cos(a); py=c+r*math.sin(a)
    # tangent direction (clockwise in screen coords = increasing angle)
    tx=-math.sin(a)*direction; ty=math.cos(a)*direction
    nx=math.cos(a); ny=math.sin(a)
    L=230; W=210
    tip=(px+tx*L*0.75, py+ty*L*0.75)
    b1=(px-tx*L*0.25+nx*W/2, py-ty*L*0.25+ny*W/2)
    b2=(px-tx*L*0.25-nx*W/2, py-ty*L*0.25-ny*W/2)
    d.polygon([tip,b1,b2],fill=col)
arc(195,315); head(315,1)
arc(15,135); head(135,1)
im.save("icon-1024.png")
