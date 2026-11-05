export interface TextSpan {text:string;kind?:'hex'}
/** UTF-8 validation keeps incomplete code points between reads and exposes invalid bytes exactly. */
export class TerminalBytes {
 private tail:number[]=[]
 feed(bytes:number[],final=false):TextSpan[]{
  const input=[...this.tail,...bytes];this.tail=[];const spans:TextSpan[]=[];let valid:number[]=[],invalid:number[]=[]
  const flush=()=>{if(valid.length){spans.push({text:new TextDecoder('utf-8',{ignoreBOM:true}).decode(new Uint8Array(valid))});valid=[]}if(invalid.length){spans.push({text:'['+invalid.map(b=>b.toString(16).padStart(2,'0')).join(' ')+']',kind:'hex'});invalid=[]}}
  for(let i=0;i<input.length;){
   const lead=input[i]!;const length=lead<128?1:lead>=0xc2&&lead<=0xdf?2:lead>=0xe0&&lead<=0xef?3:lead>=0xf0&&lead<=0xf4?4:0
   let good=length>0
   for(let j=1;j<length&&i+j<input.length;j++){const b=input[i+j]!;if(b<0x80||b>0xbf||(j===1&&((lead===0xe0&&b<0xa0)||(lead===0xed&&b>0x9f)||(lead===0xf0&&b<0x90)||(lead===0xf4&&b>0x8f))))good=false}
   if(good&&i+length>input.length&&!final){flush();this.tail=input.slice(i);break}
   if(!good||i+length>input.length){if(valid.length)flush();invalid.push(lead);i++}else{if(invalid.length)flush();for(let j=0;j<length;j++)valid.push(input[i+j]!);i+=length}
  }
  flush();return spans
 }
}
