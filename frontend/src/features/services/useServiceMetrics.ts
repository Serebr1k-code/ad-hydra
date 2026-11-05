import { onMounted, onUnmounted, ref, watch, type Ref } from 'vue'
import { api, errorMessage } from '@/shared/api/client'
import { metricPoint, retainPoint, type MetricPoint, type MetricSample, type ProxyMetrics } from './metrics'
export function useServiceMetrics(ids:Ref<string[]>) {
  const points=ref<Record<string,MetricPoint[]>>({}),status=ref('Connecting'),error=ref('')
  const forwardingStats=ref<ProxyMetrics['forwarding_stats']>(null)
  const available=ref<Record<string,boolean>>({})
  const previous=new Map<string,MetricSample>()
  let timer:ReturnType<typeof setTimeout>|undefined, busy=false, stopped=false, generation=0
  watch(ids,values=>{const retained=new Set(values);for(const id of Object.keys(points.value))if(!retained.has(id)){delete points.value[id];delete available.value[id];previous.delete(id)}})
  async function poll() {
    if(stopped || document.hidden || busy)return
    busy=true
    const requestGeneration=generation
    try {
      const response=await api<ProxyMetrics>('/system/proxy')
      if(stopped || document.hidden || requestGeneration!==generation)return
      if(!response.connected){status.value='Offline · Proxy disconnected';previous.clear();return}
      if(!response.service_metrics){status.value='Service metrics unavailable';previous.clear();return}
      status.value='Live · every 2 seconds';error.value='';forwardingStats.value=response.forwarding_stats
      const at=Date.now()
      for(const id of ids.value) {
        const metric=response.service_metrics.find(item=>item.service_id===id)
        available.value[id]=Boolean(metric)
        if(!metric){previous.delete(id);continue}
        const sample={at,epoch:response.proxy_epoch_id,metric}
        points.value[id]=retainPoint(points.value[id]??[],metricPoint(sample,previous.get(id)))
        previous.set(id,sample)
      }
    } catch(cause) {if(!stopped){status.value='Offline · status request failed';error.value=errorMessage(cause);previous.clear()}}
    finally {busy=false;if(!stopped&&!document.hidden)timer=setTimeout(poll,2000)}
  }
  function visibility() {generation++;clearTimeout(timer);previous.clear();if(document.hidden)status.value='Paused · page hidden';else {status.value='Connecting · gap after pause';void poll()}}
  onMounted(()=>{document.addEventListener('visibilitychange',visibility);void poll()})
  onUnmounted(()=>{stopped=true;clearTimeout(timer);document.removeEventListener('visibilitychange',visibility)})
  return {points,status,error,available,forwardingStats}
}
