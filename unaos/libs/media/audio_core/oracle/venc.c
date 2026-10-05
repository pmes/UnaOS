/* venc in.raw(s16le interleaved) channels rate quality out.ogg [managed_kbps] */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <vorbis/vorbisenc.h>
int main(int argc,char**argv){
  FILE*in=fopen(argv[1],"rb"); int ch=atoi(argv[2]); long rate=atol(argv[3]); float q=atof(argv[4]); FILE*out=fopen(argv[5],"wb");
  vorbis_info vi; vorbis_comment vc; vorbis_dsp_state vd; vorbis_block vb; ogg_stream_state os; ogg_page og; ogg_packet op;
  vorbis_info_init(&vi);
  int r = argc>6 ? vorbis_encode_init(&vi,ch,rate,-1,atoi(argv[6])*1000,-1) : vorbis_encode_init_vbr(&vi,ch,rate,q);
  if(r){fprintf(stderr,"init failed %d\n",r);return 1;}
  vorbis_comment_init(&vc); vorbis_comment_add_tag(&vc,"ENCODER","UnaOS AUDIOCODEC vector");
  vorbis_analysis_init(&vd,&vi); vorbis_block_init(&vd,&vb); ogg_stream_init(&os,1234);
  { ogg_packet h,hc,hcode; vorbis_analysis_headerout(&vd,&vc,&h,&hc,&hcode); ogg_stream_packetin(&os,&h); ogg_stream_packetin(&os,&hc); ogg_stream_packetin(&os,&hcode);
    while(ogg_stream_flush(&os,&og)){fwrite(og.header,1,og.header_len,out);fwrite(og.body,1,og.body_len,out);} }
  short buf[1024*16]; int eos=0;
  while(!eos){
    long n=fread(buf,2*ch,1024,in);
    if(n==0){ vorbis_analysis_wrote(&vd,0); }
    else { float**b=vorbis_analysis_buffer(&vd,n); for(long i=0;i<n;i++) for(int c=0;c<ch;c++) b[c][i]=buf[i*ch+c]/32768.f; vorbis_analysis_wrote(&vd,n); }
    while(vorbis_analysis_blockout(&vd,&vb)==1){ vorbis_analysis(&vb,NULL); vorbis_bitrate_addblock(&vb);
      while(vorbis_bitrate_flushpacket(&vd,&op)){ ogg_stream_packetin(&os,&op);
        while(!eos){ int result=ogg_stream_pageout(&os,&og); if(!result)break; fwrite(og.header,1,og.header_len,out);fwrite(og.body,1,og.body_len,out); if(ogg_page_eos(&og))eos=1; } } }
  }
  ogg_stream_clear(&os); vorbis_block_clear(&vb); vorbis_dsp_clear(&vd); vorbis_comment_clear(&vc); vorbis_info_clear(&vi); fclose(out); return 0;
}
