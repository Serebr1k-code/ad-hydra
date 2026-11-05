<script setup lang="ts">
import { ref } from 'vue'
import { Star } from 'lucide-vue-next'
import { Button } from '@/components/ui/button'
import { api, errorMessage } from '@/shared/api/client'

export type FavoriteChange = { flow_id: string; favorite: boolean; raw_history_state?: string; decoded_incomplete?: boolean }
const props = defineProps<{ flowId: string; favorite: boolean; compact?: boolean }>()
const emit = defineEmits<{ changed: [value: FavoriteChange]; error: [message: string] }>()
const saving = ref(false)
async function toggle() {
  if (saving.value) return
  saving.value = true
  emit('error', '')
  try {
    const result = await api<FavoriteChange>(`/traffic/flows/${encodeURIComponent(props.flowId)}/favorite`, 'PUT', { favorite: !props.favorite })
    emit('changed', result)
  } catch (cause) { emit('error', errorMessage(cause)) }
  finally { saving.value = false }
}
</script>

<template>
  <Button type="button" variant="ghost" size="sm" :class="compact ? 'size-8 p-0' : ''" :disabled="saving" :aria-pressed="favorite" :aria-label="favorite ? 'Remove stream from favourites' : 'Save stream in favourites'" :title="favorite ? 'Remove from favourites; normal rotation will apply again' : 'Keep retained bytes and decoded messages after traffic rotation'" @click.stop="toggle">
    <Star class="size-4" :class="favorite ? 'fill-amber-400 text-amber-400' : ''" aria-hidden="true" />
    <span v-if="!compact">{{ saving ? 'Saving…' : favorite ? 'Saved to favourites' : 'Favourite' }}</span>
  </Button>
</template>
