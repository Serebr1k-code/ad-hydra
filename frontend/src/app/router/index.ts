import { createRouter, createWebHashHistory } from 'vue-router'
export const router = createRouter({
 history:createWebHashHistory(),
 routes:[
  {path:'/',redirect:'/services'},
  {path:'/services',component:()=>import('@/features/services/ServicesPage.vue')},
  {path:'/filters',component:()=>import('@/features/filters/FiltersPage.vue')},
  {path:'/traffic',component:()=>import('@/features/traffic/TrafficPage.vue')},
  {path:'/protocols',component:()=>import('@/features/protocols/ProtocolsPage.vue')},
  {path:'/farm',component:()=>import('@/features/farm/FarmPage.vue')},
  {path:'/flags',component:()=>import('@/features/flags/FlagsPage.vue')},
  {path:'/users',component:()=>import('@/features/users/UsersPage.vue')},
  {path:'/settings',component:()=>import('@/features/settings/SettingsPage.vue')},
  {path:'/:pathMatch(.*)*',redirect:'/services'},
 ],
})
