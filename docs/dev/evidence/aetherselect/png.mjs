import zlib from 'zlib';
export function decode(buf){let p=8,w,h,ct,idat=[];while(p<buf.length){const len=buf.readUInt32BE(p);const t=buf.toString('ascii',p+4,p+8);const d=buf.subarray(p+8,p+8+len);if(t==='IHDR'){w=d.readUInt32BE(0);h=d.readUInt32BE(4);ct=d[9];}else if(t==='IDAT')idat.push(d);p+=12+len;}
const raw=zlib.inflateSync(Buffer.concat(idat));const bpp=ct===6?4:3;const st=w*bpp;const out=Buffer.alloc(h*st);
for(let y=0;y<h;y++){const f=raw[y*(st+1)];const line=raw.subarray(y*(st+1)+1,(y+1)*(st+1));for(let x=0;x<st;x++){const a=x>=bpp?out[y*st+x-bpp]:0;const b=y>0?out[(y-1)*st+x]:0;const c=(x>=bpp&&y>0)?out[(y-1)*st+x-bpp]:0;let v=line[x];if(f===1)v+=a;else if(f===2)v+=b;else if(f===3)v+=(a+b)>>1;else if(f===4){const pp=a+b-c,pa=Math.abs(pp-a),pb=Math.abs(pp-b),pc=Math.abs(pp-c);v+=(pa<=pb&&pa<=pc)?a:(pb<=pc?b:c);}out[y*st+x]=v&255;}}
return {w,h,bpp,px:(x,y)=>[...out.subarray((y*w+x)*bpp,(y*w+x)*bpp+3)]};}
