import type {EffectivenessLatestRun} from '@/generated/contracts'
export type {EffectivenessRow,EffectivenessTeam,EffectivenessLatestRun,EffectivenessCell,EffectivenessMatrix} from '@/generated/contracts'
export function matrixCellKey(exploitId:string,serviceId:string|null|undefined,teamKey:string):string {
  return JSON.stringify([exploitId,serviceId??null,teamKey])
}
export function targetAttemptDuration(latest:EffectivenessLatestRun,nowMillis:number):string {
  if(latest.process_started_at==null)return 'Unknown target duration'
  const end=latest.process_finished_at??(latest.process_status==='running'?nowMillis:null)
  if(end===null)return 'Unknown target duration'
  const millis=Math.max(0,end-latest.process_started_at)
  return millis<1000?`${millis} ms`:`${(millis/1000).toFixed(1)} s`
}
