import {expect,test} from 'bun:test'
import {matrixCellKey,targetAttemptDuration,type EffectivenessLatestRun} from './effectivenessTypes'
const latest:EffectivenessLatestRun={id:'run',status:'succeeded',mode:'batch',created_at:100,started_at:101,finished_at:102,exit_code:0,process_status:null,process_started_at:null,process_finished_at:null,process_exit_code:null}
test('batch aggregate success does not invent target duration',()=>{expect(targetAttemptDuration(latest,10000)).toBe('Unknown target duration')})
test('actual target attempt duration uses milliseconds and terminal timing',()=>{
 expect(targetAttemptDuration({...latest,process_status:'running',process_started_at:1000},1500)).toBe('500 ms')
 expect(targetAttemptDuration({...latest,process_status:'failed',process_started_at:1000,process_finished_at:2456},10000)).toBe('1.5 s')
})
test('matrix keys distinguish service assignments and legacy identity namespaces',()=>{
 expect(matrixCellKey('a',null,'target:b')).not.toBe(matrixCellKey('a','web','team:b'))
 expect(matrixCellKey('a:b','c','d')).not.toBe(matrixCellKey('a','b:c','d'))
})
