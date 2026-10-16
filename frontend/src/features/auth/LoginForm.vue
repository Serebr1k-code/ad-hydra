<script setup lang="ts">
import { ref } from 'vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
defineProps<{busy:boolean}>()
const emit = defineEmits<{submit:[username:string,password:string]}>()
const username = ref(''), password = ref('')
function submit() { emit('submit',username.value,password.value); password.value = '' }
</script>
<template>
 <section id="login" class="mx-auto my-16 max-w-sm space-y-5">
  <h1>Sign in</h1><form id="login-form" @submit.prevent="submit" class="space-y-4">
   <div class="space-y-2"><Label for="username">Username</Label><Input id="username" name="username" v-model="username" autocomplete="username" required :disabled="busy"/></div>
   <div class="space-y-2"><Label for="password">Password</Label><Input id="password" name="password" v-model="password" type="password" autocomplete="current-password" required :disabled="busy"/></div>
   <Button type="submit" class="w-full" :disabled="busy">{{busy?'Signing in…':'Sign in'}}</Button>
  </form><p class="text-sm text-muted-foreground">Accounts are created with the Hydra CLI.</p>
 </section>
</template>
