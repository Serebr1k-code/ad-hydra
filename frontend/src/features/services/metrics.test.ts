import { expect, test } from 'bun:test'
import { chartPaths, metricPoint, retainPoint, type MetricSample } from './metrics'
import { recommendedProtection, validateProtection } from './protection'
function sample(at:number,count:number,epoch='one'):MetricSample {return {at,epoch,metric:{service_id:'a',active_flows:4,accepted_flows:count,rejected_flows:count,bytes_client_to_server:count*10,bytes_server_to_client:count*20,dropped_datagrams:count,rate_limited:0}}}
test('rates use elapsed time and keep both directions independent',()=>{
  const point=metricPoint(sample(3500,15),sample(1000,5))
  expect(point.accepted).toBe(4);expect(point.upload).toBe(40);expect(point.download).toBe(80);expect(point.active).toBe(4)
})
test('initial samples, outages, epoch changes and counter rollback never invent rates',()=>{
  expect(metricPoint(sample(1000,3)).accepted).toBeNull()
  expect(metricPoint(sample(10000,10),sample(1000,1)).state).toBe('gap')
  expect(metricPoint(sample(3000,10,'two'),sample(1000,1)).state).toBe('reset')
  expect(metricPoint(sample(3000,0),sample(1000,1)).accepted).toBeNull()
})
test('retention is bounded and paths break across missing baselines',()=>{
  let points=Array.from({length:120},(_,i)=>metricPoint(sample(i*2000,i),i?sample((i-1)*2000,i-1):undefined))
  points=retainPoint(points,metricPoint(sample(250000,0,'two'),sample(238000,119)))
  expect(points.length).toBe(120)
  const resumed=metricPoint(sample(252000,1,'two'),sample(250000,0,'two'))
  expect(chartPaths([...points,resumed],'active').length).toBe(2)
})
test('protection accepts disabled and rate-zero states but rejects invalid caps',()=>{
  expect(validateProtection(null)).toBe('')
  expect(validateProtection({...recommendedProtection(),bytes_per_second:0})).toBe('')
  expect(validateProtection({...recommendedProtection(),max_active_flows:0})).not.toBe('')
  expect(validateProtection({...recommendedProtection(),idle_timeout_ms:NaN})).not.toBe('')
  expect(validateProtection({...recommendedProtection(),bytes_per_second:0.5})).not.toBe('')
})
