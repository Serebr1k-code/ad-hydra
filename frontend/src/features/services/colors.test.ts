import { expect,test } from 'bun:test'
import { servicePalette,resolveServiceColors,validServiceColor } from './colors'
test('twelve presets remain distinct for unconfigured services independent of list order',()=>{
 const services=Array.from({length:12},(_,i)=>({id:`service-${i}`}))
 const colors=resolveServiceColors(services)
 expect(servicePalette.length).toBeGreaterThanOrEqual(10)
 expect(new Set(Object.values(colors)).size).toBe(12)
 expect(resolveServiceColors([...services].reverse())).toEqual(colors)
})
test('configured colors are honored and never interpreted as CSS expressions',()=>{
 const colors=resolveServiceColors([{id:'one',color:'#AB12CD'},{id:'two',color:'url(example)'}])
 expect(colors.one).toBe('#ab12cd')
 expect(validServiceColor(colors.two)).toBe(true)
 expect(validServiceColor('#123')).toBe(false)
 expect(validServiceColor('red')).toBe(false)
})
