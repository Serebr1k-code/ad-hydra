<script setup lang="ts">
import {ref} from 'vue'
import {api,errorMessage} from '@/shared/api/client'
import type {User} from '@/generated/contracts'
import {Button} from '@/components/ui/button'
import {Input} from '@/components/ui/input'
import {Label} from '@/components/ui/label'
const emit=defineEmits<{created:[user:User]}>()
const name=ref(''),password=ref(''),busy=ref(false),error=ref(''),notice=ref('')
async function create(){
 busy.value=true;error.value='';notice.value=''
 try{
  const bytes=new TextEncoder().encode(password.value).length
  if(bytes<12||bytes>1024)throw new Error('Password must contain 12 to 1024 bytes.')
  const user=await api<User>('/users','POST',{username:name.value,password:password.value})
  name.value='';password.value='';notice.value=`Created ${user.username}.`;emit('created',user)
 }catch(cause){error.value=errorMessage(cause)}finally{busy.value=false}
}
</script>
<template>
 <form id="create-user-form" class="space-y-3 rounded border p-4" @submit.prevent="create">
  <h2 class="text-lg font-semibold">Add user</h2>
  <p class="text-sm text-muted-foreground">New users have the same full privileges as every other account.</p>
  <div><Label for="create-user-name">Username</Label><Input id="create-user-name" v-model="name" required maxlength="64" autocomplete="off" :disabled="busy" /></div>
  <div><Label for="create-user-password">Password</Label><Input id="create-user-password" v-model="password" type="password" autocomplete="new-password" required :disabled="busy" /></div>
  <Button type="submit" :disabled="busy">{{busy?'Creating…':'Create user'}}</Button>
  <p v-if="error" role="alert" class="text-destructive">{{error}}</p><p v-if="notice" role="status">{{notice}}</p>
 </form>
</template>
