import {record,type Message,type SavedHistorySegment} from './traffic'
import {TerminalBytes} from './terminalBytes'
export interface TerminalChunk {id:string;direction:number;text:string;kind?:'hex'}
export interface TerminalTranscript {chunks:TerminalChunk[];ordered:boolean;truncated:boolean}
const LIMIT=256*1024
function bounded(chunks:TerminalChunk[],ordered=true):TerminalTranscript {
 let size=0;const retained:TerminalChunk[]=[];let truncated=false
 for(let i=chunks.length-1;i>=0;i--){const chunk=chunks[i]!;const remaining=LIMIT-size;if(remaining<=0){truncated=true;break}if(chunk.text.length>remaining){retained.unshift({...chunk,text:chunk.text.slice(-remaining)});truncated=true;break}retained.unshift(chunk);size+=chunk.text.length}
 return {chunks:retained,ordered,truncated}
}
export function terminalMessages(messages:Message[],side:'both'|'1'|'2'):TerminalTranscript {
 const chunks:TerminalChunk[]=[]
 for(const message of messages){if(side!=='both'&&String(message.direction)!==side)continue
  const spans=record(message.fields['utf8.spans']);if(spans.type==='list'&&Array.isArray(spans.value)){for(const [index,item]of spans.value.entries()){const span=record(item);if(span.type==='string'&&typeof span.value==='string')chunks.push({id:message.message_id+':'+index,direction:Number(message.direction),text:span.value});else if(span.type==='bytes'&&Array.isArray(span.value))chunks.push({id:message.message_id+':'+index,direction:Number(message.direction),text:'['+span.value.map((b:number)=>b.toString(16).padStart(2,'0')).join(' ')+']',kind:'hex'})}}else{const field=record(message.fields['utf8.text']);if(field.type==='string'&&typeof field.value==='string')chunks.push({id:message.message_id,direction:Number(message.direction),text:field.value})}
 }return bounded(chunks)
}
export function terminalHistory(segments:SavedHistorySegment[],side:'both'|'1'|'2'):TerminalTranscript {
 const populated=segments.filter(s=>s.data.length>0)
 const ordered=populated.length<2||populated.every(s=>Number(s.sequence)>0)
 const decoders=new Map<number,TerminalBytes>();const chunks:TerminalChunk[]=[]
 for(const [index,segment]of segments.entries()){
  if(side!=='both'&&String(segment.direction)!==side)continue
  let decoder=decoders.get(segment.direction);if(!decoder){decoder=new TerminalBytes();decoders.set(segment.direction,decoder)}
  for(const [part,span]of decoder.feed(segment.data,segment.datagram).entries())chunks.push({id:'history:'+index+':'+part,direction:segment.direction,...span})
 }
 for(const [direction,decoder]of decoders){for(const [part,span]of decoder.feed([],true).entries())chunks.push({id:'tail:'+direction+':'+part,direction,...span})}
 return bounded(chunks,ordered)
}
