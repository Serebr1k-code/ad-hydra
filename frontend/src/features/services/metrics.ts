export interface ServiceMetric {
  service_id:string; active_flows:number; accepted_flows:number; rejected_flows:number
  bytes_client_to_server:number; bytes_server_to_client:number; dropped_datagrams:number; rate_limited:number
}
export interface ProxyMetrics {
 forwarding_stats?: {mappings:number;reused:number;restored:number;errors:number}|null; connected:boolean; proxy_epoch_id:string; service_metrics:ServiceMetric[] }
export type MetricKey = 'active'|'accepted'|'rejected'|'upload'|'download'|'drops'
export interface MetricPoint { at:number; active:number; accepted:number|null; rejected:number|null; upload:number|null; download:number|null; drops:number|null; state:'live'|'baseline'|'gap'|'reset' }
export interface MetricSample { at:number; epoch:string; metric:ServiceMetric }
const counters = ['accepted_flows','rejected_flows','bytes_client_to_server','bytes_server_to_client','dropped_datagrams'] as const
export function metricPoint(current:MetricSample, previous?:MetricSample):MetricPoint {
  const elapsed = previous ? (current.at-previous.at)/1000 : 0
  const reset = previous && (current.epoch !== previous.epoch || counters.some(key => current.metric[key] < previous.metric[key]))
  const state = reset ? 'reset' : !previous ? 'baseline' : elapsed > 6 || elapsed <= 0 ? 'gap' : 'live'
  const rate = (key:typeof counters[number]) => state === 'live' && previous ? (current.metric[key]-previous.metric[key])/elapsed : null
  return {at:current.at,active:current.metric.active_flows,accepted:rate('accepted_flows'),rejected:rate('rejected_flows'),upload:rate('bytes_client_to_server'),download:rate('bytes_server_to_client'),drops:rate('dropped_datagrams'),state}
}
export function retainPoint(points:MetricPoint[], point:MetricPoint):MetricPoint[] { return [...points,point].slice(-120) }
export function chartPaths(points:MetricPoint[], key:MetricKey):string[] {
  const max=Math.max(1,...points.map(point=>point[key]??0)),start=points[0]?.at??0,end=points.at(-1)?.at??start
  const paths:string[]=[];let path=''
  points.forEach((point,index)=>{
    const value=point[key]
    if (value === null || (index>0 && point.state !== 'live')) {if(path)paths.push(path);path=''}
    if(value !== null) {const x=end===start?0:(point.at-start)/(end-start)*600,y=90-value/max*80;path+=`${path?' L':'M'}${x.toFixed(2)},${y.toFixed(2)}`}
  })
  if(path)paths.push(path)
  return paths
}
