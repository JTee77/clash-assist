from PIL import Image, ImageDraw, ImageFont
S=2;W0,H0=540,360;W,H=W0*S,H0*S
img=Image.new("RGB",(W,H));d=ImageDraw.Draw(img)
for y in range(H):
    t=y/H; c=tuple(int(a+(b-a)*t) for a,b in zip((242,239,255),(250,249,253))); d.line([(0,y),(W,y)],fill=c)
for x in range(W):
    t=x/W; c=tuple(int(a+(b-a)*t) for a,b in zip((111,84,253),(152,173,253))); d.line([(x,0),(x,4*S)],fill=c)
B="/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc";R="/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"
f=lambda p,s:ImageFont.truetype(p,int(s*S),index=2)
def ctext(x,y,t,ft,fill):
    w=d.textlength(t,font=ft); d.text((x-w/2,y),t,font=ft,fill=fill)
ctext(W/2,26*S,"拖到“应用程序”即可安装",f(B,19),"#1F1B3A")
for cx in (140,400):
    r=56*S; d.ellipse([cx*S-r,135*S-r,cx*S+r,135*S+r],fill="#ECE8FD")
x0,x1,y=212*S,328*S,135*S
for x in range(x0,x1-16*S):
    t=(x-x0)/(x1-x0); c=tuple(int(a+(b-a)*t) for a,b in zip((122,64,240),(119,143,253)))
    d.line([(x,y-3*S),(x,y+3*S)],fill=c)
d.polygon([(x1,y),(x1-20*S,y-13*S),(x1-20*S,y+13*S)],fill=(125,150,253))
d.rounded_rectangle([24*S,228*S,516*S,344*S],radius=10*S,fill="#FFFFFF",outline="#E2DCF7",width=S)
d.text((40*S,238*S),"首次打开提示（本应用未经公证）",font=f(B,12),fill="#6F54FD")
steps=["1. 首次打开弹窗时点「完成」（不要点「移到废纸篓」）",
"2. 打开「系统设置 › 隐私与安全性」，拉到底点「仍要打开」",
"3. 确认框里再点「仍要打开」，按提示完成身份验证"]
for i,s in enumerate(steps): d.text((40*S,(262+i*24)*S),s,font=f(R,12),fill="#374151")
img.save("dmg-background@2x.png",dpi=(144,144))
img.resize((W0,H0),Image.LANCZOS).save("dmg-background.png",dpi=(72,72))
# preview with icons
p=img.copy()
ic=Image.open("/tmp/icon.png").convert("RGBA").resize((96*S,96*S)) if __import__("os").path.exists("/tmp/icon.png") else None
if ic: p.paste(ic,(140*S-48*S,135*S-48*S),ic)
p.save("preview@2x.png")
