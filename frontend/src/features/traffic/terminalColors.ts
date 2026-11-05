import type {TerminalChunk} from './terminal'
const CLIENT='\x1b[38;2;248;113;113m',SERVER='\x1b[38;2;96;165;250m',HEX='\x1b[38;2;250;204;21m'
/** Keep cursor controls, consume payload SGR and string controls; never inject colors inside an escape. */
export function coloredChunks(chunks:TerminalChunk[]):TerminalChunk[]{
 let escape='',string=false,discard=false
 return chunks.map(chunk=>{
  if(chunk.kind==='hex'){escape='';string=false;discard=false;return {...chunk,text:HEX+chunk.text}}
  let text=''
  for(const char of chunk.text.replace(/[\u0080-\u009f]/g,c=>({'\u0085':'\n','\u0090':'\x1bP','\u0098':'\x1bX','\u009b':'\x1b[','\u009c':'\x1b\\','\u009d':'\x1b]','\u009e':'\x1b^','\u009f':'\x1b_'}[c]??''))){
   if(!escape){if(char==='\x1b'){escape=char;continue}text+=char;continue}
   escape+=char
   if(escape.length===2){string=']PX^_'.includes(char);if(string)continue;if(char==='[')continue}
   if(string){if(char==='\x07'||escape.endsWith('\x1b\\')){escape='';string=false;discard=false}else if(escape.length>4096){escape='\x1b]';discard=true}continue}
   if(escape[1]==='['){if(char<'@'||char>'~')continue;if(char!=='m'&&escape!=='\x1b[!p'&&!discard)text+=escape}
   else if(char>='0'&&char<='~'){if(escape!=='\x1bc'&&!discard)text+=escape}
   else continue
   escape='';discard=false
  }
  return {...chunk,text:text?(chunk.direction===1?CLIENT:SERVER)+text:''}
 })
}
