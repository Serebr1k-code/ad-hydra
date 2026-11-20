import type {TerminalChunk} from './terminal'
const CLIENT='\x1b[38;2;248;113;113m',SERVER='\x1b[38;2;96;165;250m',HEX='\x1b[38;2;250;204;21m'
interface ParserState {escape:string;string:boolean;discard:boolean}
/** Keep cursor controls, consume payload SGR and string controls; never inject colors inside an escape. */
export function coloredChunks(chunks:TerminalChunk[]):TerminalChunk[]{
 const states=new Map<number,ParserState>()
 return chunks.map(chunk=>{
  let state=states.get(chunk.direction)
  if(!state){state={escape:'',string:false,discard:false};states.set(chunk.direction,state)}
  if(chunk.kind==='hex'){state.escape='';state.string=false;state.discard=false;return {...chunk,text:HEX+chunk.text}}
  let text=''
  for(const char of chunk.text.replace(/[\u0080-\u009f]/g,c=>({'\u0085':'\n','\u0090':'\x1bP','\u0098':'\x1bX','\u009b':'\x1b[','\u009c':'\x1b\\','\u009d':'\x1b]','\u009e':'\x1b^','\u009f':'\x1b_'}[c]??''))){
   if(!state.escape){if(char==='\x1b'){state.escape=char;continue}text+=char;continue}
   state.escape+=char
   if(state.escape.length===2){state.string=']PX^_'.includes(char);if(state.string)continue;if(char==='[')continue}
   if(state.string){if(char==='\x07'||state.escape.endsWith('\x1b\\')){state.escape='';state.string=false;state.discard=false}else if(state.escape.length>4096){state.escape='\x1b]';state.discard=true}continue}
   if(state.escape[1]==='['){if(char<'@'||char>'~')continue;if(char!=='m'&&state.escape!=='\x1b[!p'&&!state.discard)text+=state.escape}
   else if(char>='0'&&char<='~'){if(state.escape!=='\x1bc'&&!state.discard)text+=state.escape}
   else continue
   state.escape='';state.discard=false
  }
  return {...chunk,text:text?(chunk.direction===1?CLIENT:SERVER)+text:''}
 })
}
