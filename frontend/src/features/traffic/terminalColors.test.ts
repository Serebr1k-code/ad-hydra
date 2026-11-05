import {expect,test} from 'bun:test'
import {coloredChunks} from './terminalColors'
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
