import type {FarmRun} from './types'
export function runDuration(run:FarmRun,nowSeconds:number):string {
 if(run.started_at==null)return 'Not started'
 const end=run.finished_at??(run.status==='running'?nowSeconds:null)
 if(end===null)return 'Duration unavailable'
 const seconds=Math.max(0,Math.floor(end-run.started_at))
 const hours=Math.floor(seconds/3600),minutes=Math.floor(seconds%3600/60),rest=seconds%60
 return `${hours?`${hours}h `:''}${hours||minutes?`${minutes}m `:''}${rest}s`
}
