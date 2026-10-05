/* vdec in.ogg out.f32 : libvorbis (vorbisfile) float decode, interleaved f32le; prints ch rate frames */
#include <stdio.h>
#include <vorbis/vorbisfile.h>
int main(int argc,char**argv){
  OggVorbis_File vf; if(ov_fopen(argv[1],&vf)){fprintf(stderr,"open failed\n");return 1;}
  vorbis_info*vi=ov_info(&vf,-1); FILE*out=fopen(argv[2],"wb"); long tot=0; int bs;
  for(;;){ float**pcm; long n=ov_read_float(&vf,&pcm,4096,&bs); if(n<=0){ if(n<0) {fprintf(stderr,"hole %ld\n",n); continue;} break; }
    for(long i=0;i<n;i++) for(int c=0;c<vi->channels;c++) fwrite(&pcm[c][i],4,1,out); tot+=n; }
  printf("%d %ld %ld\n",vi->channels,vi->rate,tot); fclose(out); ov_clear(&vf); return 0;
}
