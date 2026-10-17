import {expect,test} from 'bun:test'
import {runDuration} from './runTime'
import type {FarmRun} from './types'
const run:FarmRun={id:'r',exploit_id:'e',status:'running',created_at:100,started_at:110,initiated_by:'u',version:'1'}
test('run duration excludes queue time, advances while running and stops at finish',()=>{
 expect(runDuration(run,175)).toBe('1m 5s')
 expect(runDuration({...run,finished_at:123,status:'succeeded'},1000)).toBe('13s')
 expect(runDuration({...run,started_at:null,status:'queued'},175)).toBe('Not started')
 expect(runDuration({...run,status:'interrupted'},175)).toBe('Duration unavailable')
 expect(runDuration({...run,finished_at:110+3661},1000)).toBe('1h 1m 1s')
})
