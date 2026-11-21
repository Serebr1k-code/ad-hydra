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
/** Amber-500 wash matching the "Flag detected" badge, over the terminal background. */
const FLAG_BACKGROUND='\x1b[48;2;61;46;18m',FLAG_RESET='\x1b[49m'
const FLAG_SCAN_LIMIT=64*1024,FLAG_MATCH_LIMIT=256
/**
 * Wash every flag match with the badge background without touching the text colour
 * that direction colouring already applied. Runs after `coloredChunks`, because that
 * pass consumes payload SGR sequences.
 */
export function flagChunks(chunks:TerminalChunk[],pattern:RegExp|null):TerminalChunk[]{
 if(!pattern)return chunks
 const starts=new Map<number,number>()
 let remaining=FLAG_SCAN_LIMIT
 for(let i=chunks.length-1;i>=0&&remaining>0;i--){
  const chunk=chunks[i]!
  if(chunk.kind==='hex')continue
  const start=chunk.text.length<=remaining?0:safeTextStart(chunk.text,chunk.text.length-remaining)
  starts.set(i,start);remaining-=chunk.text.length-start
 }
 return chunks.map((chunk,index)=>{
  if(chunk.kind==='hex'||!starts.has(index))return chunk
  const start=starts.get(index)!,tail=chunk.text.slice(start)
  const highlighted=highlightTextOutsideAnsi(tail,pattern)
  return highlighted===tail?chunk:{...chunk,text:chunk.text.slice(0,start)+highlighted}
 })
}

function highlightTextOutsideAnsi(text:string,pattern:RegExp):string{
 let result='',plainStart=0,scan=0,matchCount=0
 while(scan<text.length){
  if(text[scan]!=='\x1b'){scan++;continue}
  result+=highlightPlain(text.slice(plainStart,scan),pattern,()=>matchCount,(count)=>{matchCount=count})
  const end=controlEnd(text,scan)
  result+=text.slice(scan,end);scan=end;plainStart=end
 }
 result+=highlightPlain(text.slice(plainStart),pattern,()=>matchCount,(count)=>{matchCount=count})
 return result
}

function highlightPlain(text:string,pattern:RegExp,getCount:()=>number,setCount:(count:number)=>void):string{
 if(!text||getCount()>=FLAG_MATCH_LIMIT)return text
 pattern.lastIndex=0
 let result='',cursor=0
 while(getCount()<FLAG_MATCH_LIMIT){
  const match=pattern.exec(text)
  if(!match)break
  if(!match[0]){pattern.lastIndex++;continue}
  result+=text.slice(cursor,match.index)+FLAG_BACKGROUND+match[0]+FLAG_RESET
  cursor=match.index+match[0].length;setCount(getCount()+1)
 }
 pattern.lastIndex=0
 return result?result+text.slice(cursor):text
}

/** Find the next complete terminal control boundary; never inspect bytes inside it. */
function controlEnd(text:string,start:number):number{
 const kind=text[start+1]
 if(!kind)return text.length
 if(kind==='['){for(let i=start+2;i<text.length;i++){const code=text.charCodeAt(i);if(code>=0x40&&code<=0x7e)return i+1}return text.length}
 if(']PX^_'.includes(kind)){for(let i=start+2;i<text.length;i++){if(text[i]==='\x07')return i+1;if(text[i]==='\x1b'&&text[i+1]==='\\')return i+2}return text.length}
 return Math.min(text.length,start+2)
}

/** If a bounded scan begins inside a control, start after it instead. */
function safeTextStart(text:string,start:number):number{
 for(let i=0;i<start;i++)if(text[i]==='\x1b'){const end=controlEnd(text,i);if(end>start)return end;i=end-1}
 return start
}
