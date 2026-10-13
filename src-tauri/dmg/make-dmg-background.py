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
ctext(W/2,12*S,"拖到“应用程序”即可安装",f(B,17),"#1F1B3A")
CY=92
for cx in (140,400):
    r=50*S; d.ellipse([cx*S-r,CY*S-r,cx*S+r,CY*S+r],fill="#ECE8FD")
x0,x1,y=212*S,328*S,CY*S
for x in range(x0,x1-16*S):
    t=(x-x0)/(x1-x0); c=tuple(int(a+(b-a)*t) for a,b in zip((122,64,240),(119,143,253)))
    d.line([(x,y-3*S),(x,y+3*S)],fill=c)
d.polygon([(x1,y),(x1-20*S,y-13*S),(x1-20*S,y+13*S)],fill=(125,150,253))
d.rounded_rectangle([24*S,168*S,516*S,252*S],radius=10*S,fill="#FFFFFF",outline="#E2DCF7",width=S)
d.text((38*S,174*S),"首次打开提示（本应用未经公证）",font=f(B,11),fill="#6F54FD")
steps=["1. 首次打开弹窗时点「完成」（不要点「移到废纸篓」）",
"2. 打开「系统设置 › 隐私与安全性」，拉到底点「仍要打开」",
"3. 确认框里再点「仍要打开」，按提示完成身份验证"]
for i,s in enumerate(steps): d.text((38*S,(193+i*18)*S),s,font=f(R,11),fill="#374151")
img.save("dmg-background@2x.png",dpi=(144,144))
img.resize((W0,H0),Image.LANCZOS).save("dmg-background.png",dpi=(72,72))
# simulate both bars (bottom ~90pt) + icons
p=img.copy();dd=ImageDraw.Draw(p);dd.rectangle([0,(360-90)*S,W,H],fill="#4A4A50")
for cx in (140,400): dd.rounded_rectangle([(cx-48)*S,(CY-48)*S,(cx+48)*S,(CY+48)*S],radius=20*S,fill="#FFFFFF" if cx==140 else "#6CC4F0")
for cx,t in ((140,"Clash Assist.app"),(400,"Applications")):
    w=dd.textlength(t,font=f(R,12));dd.text((cx*S-w/2,(CY+50)*S),t,font=f(R,12),fill="#111")
p.save("sim-both-bars.png")
