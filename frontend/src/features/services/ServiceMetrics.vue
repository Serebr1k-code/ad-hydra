<script setup lang="ts">
import { computed } from 'vue'
import ServiceMetricChart from './ServiceMetricChart.vue'
import { useServiceMetrics } from './useServiceMetrics'
const props=defineProps<{services:{id:string;name?:string;transport:string;listen_address:string}[];colors:Record<string,string>}>()
const ids=computed(()=>props.services.map(service=>service.id))
const {points,status,error,available,forwardingStats}=useServiceMetrics(ids)
</script>
<template>
  <section v-if="services.length" class="space-y-3" aria-label="Live service metrics">
    <div><h2 class="text-lg font-semibold">Live service traffic</h2><p role="status" class="text-sm text-muted-foreground">{{ status }}. Recent view since opening this page · up to 120 samples per service.</p></div>
    <p v-if="error" role="alert" class="text-sm text-destructive">{{ error }}</p>
    <p v-if="forwardingStats && (forwardingStats.mappings || forwardingStats.errors)" class="rounded-md border p-3 text-sm">Flag forwarding · {{forwardingStats.mappings}} durable mappings · {{forwardingStats.reused}} reused · {{forwardingStats.restored}} restored · {{forwardingStats.errors}} failures (all services).</p>
    <article v-for="service in services" :key="service.id" class="space-y-3 rounded-md border border-l-4 p-4" :style="{borderLeftColor:colors[service.id]}">
      <h3 class="font-medium">{{service.name||service.id}} · {{ service.transport.toUpperCase() }} · {{ service.listen_address }}</h3>
      <p v-if="available[service.id] === false" class="text-sm text-muted-foreground">Proxy has no current metrics for this service. Previous samples below are stale.</p>
      <p v-if="!points[service.id]?.length" class="text-sm text-muted-foreground">Waiting for Proxy service metrics.</p>
      <template v-else>
        <p class="text-xs text-muted-foreground">Last sample {{ new Date(points[service.id]!.at(-1)!.at).toLocaleTimeString() }} · {{ points[service.id]!.at(-1)!.state }}. {{ service.transport==='udp'?'New/rejected counts represent UDP session admissions; drops represent datagrams.':'New/rejected counts represent TCP connections.' }}</p>
        <div class="grid gap-6 lg:grid-cols-2">
          <ServiceMetricChart :points="points[service.id]!" :color="colors[service.id] ?? '#4dabf7'" title="Sessions and admissions" :series="[{key:'active',label:'Active sessions',unit:''},{key:'accepted',label:'New sessions',unit:'/s'},{key:'rejected',label:'Rejected sessions',unit:'/s'}]" />
          <ServiceMetricChart :points="points[service.id]!" :color="colors[service.id] ?? '#4dabf7'" title="Traffic and datagram loss" :series="[{key:'upload',label:'Client → server',unit:'B/s'},{key:'download',label:'Server → client',unit:'B/s'},{key:'drops',label:'UDP drops',unit:'/s'}]" />
        </div>
      </template>
    </article>
  </section>
</template>
