import {expect,test} from 'bun:test'
import {coloredChunks,flagChunks} from './terminalColors'
test('client red, server blue, invalid bytes yellow; payload colors cannot override direction',()=>{
 const result=coloredChunks([{id:'a',direction:2,text:'\x1b[32mPrompt\x1b[0m'},{id:'b',direction:1,text:'answer'},{id:'c',direction:2,kind:'hex',text:'[ff]'}])
 expect(result.map(c=>c.text)).toEqual(['\x1b[38;2;96;165;250mPrompt','\x1b[38;2;248;113;113manswer','\x1b[38;2;250;204;21m[ff]'])
})
test('fragmented ANSI never receives injected colors inside escapes; cursor controls survive',()=>{
 const result=coloredChunks([{id:'a',direction:2,text:'\x1b[3'},{id:'b',direction:2,text:'2mhello\r\x1b['},{id:'c',direction:2,text:'2Kworld\x1b]52;c;secret\x07'}])
 expect(result.map(c=>c.text).join('')).toBe('\x1b[38;2;96;165;250mhello\r\x1b[38;2;96;165;250m\x1b[2Kworld')
})
test('hex annotations remain literal when invalid bytes interrupt payload escape commands',()=>{
 for(const prefix of ['\x1b','\x1b[','\x1b]title','\x1bP']){
  const result=coloredChunks([{id:'a',direction:2,text:prefix},{id:'b',direction:2,kind:'hex',text:'[ff]'},{id:'c',direction:2,text:'X'}])
  expect(result[1]!.text).toBe('\x1b[38;2;250;204;21m[ff]');expect(result[2]!.text).toBe('\x1b[38;2;96;165;250mX')
 }
})
test('C1 commands and terminal resets cannot override direction or swallow following text',()=>{
 const result=coloredChunks([{id:'a',direction:2,text:'\u009b31mblue\u009d52;clipboard\u009ctext\x1bc\x1b[!p!'}])
 expect(result[0]!.text).toBe('\x1b[38;2;96;165;250mbluetext!')
})
test('an unfinished terminal command in one direction cannot swallow the other direction',()=>{
 const result=coloredChunks([{id:'client',direction:1,text:'\x1b]unfinished title'},{id:'server',direction:2,text:'HELLO\n'}])
 expect(result.map(chunk=>chunk.text)).toEqual(['','\x1b[38;2;96;165;250mHELLO\n'])
})
test('flag wash keeps the direction colour and resets only the background',()=>{
 const [chunk]=flagChunks(coloredChunks([{id:'a',direction:1,text:'ok FLAG{one} tail'}]),/FLAG\{[^}]+\}/g)
 expect(chunk!.text).toBe('\x1b[38;2;248;113;113mok \x1b[48;2;61;46;18mFLAG{one}\x1b[49m tail')
})
test('flag wash leaves hex spans and unmatched chunks untouched',()=>{
 const chunks=coloredChunks([{id:'h',direction:1,text:'de ad',kind:'hex'},{id:'b',direction:2,text:'plain'}])
 expect(flagChunks(chunks,/FLAG\{[^}]+\}/g)).toEqual(chunks)
})
test('flag wash is a no-op without a pattern',()=>{
 const chunks=coloredChunks([{id:'a',direction:1,text:'FLAG{one}'}])
 expect(flagChunks(chunks,null)).toBe(chunks)
})
test('flag wash highlights every match in one chunk',()=>{
 const [chunk]=flagChunks(coloredChunks([{id:'a',direction:2,text:'A=1 B=2'}]),/[AB]=\d/g)
 expect(chunk!.text.match(/\x1b\[48;2;61;46;18m/g)).toHaveLength(2)
})
test('flag wash never inserts ANSI styling inside a cursor control sequence',()=>{
 const [chunk]=flagChunks(coloredChunks([{id:'a',direction:1,text:'\x1b[2J'}]),/2/g)
 expect(chunk!.text).toBe('\x1b[38;2;248;113;113m\x1b[2J')
})
