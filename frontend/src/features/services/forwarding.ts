export type ForwardFraming = {type:'http1'|'datagram'} | {type:'delimited';delimiter:string} | {type:'length_prefixed';offset:number;width:number;little_endian:boolean;length_includes_header:boolean}
export interface ForwardingDraft {team_id:string;target_port:number;flag_pattern:string;framing:ForwardFraming}
export function forwardingDraft(value:unknown):ForwardingDraft|null {
 if(!value||typeof value!=='object')return null
 const item=value as Record<string,unknown>
 const port=typeof item.target_port==='number'?item.target_port:Number(String(item.target_address??'').split(':').at(-1))
 return {team_id:String(item.team_id??''),target_port:port,flag_pattern:String(item.flag_pattern??''),framing:{...item.framing as ForwardFraming}}
}
export function validateForwarding(value:ForwardingDraft|null):string {
 if(!value)return ''
 if(!value.team_id)return 'Select a configured team.'
 if(!Number.isInteger(value.target_port)||value.target_port<1||value.target_port>65535)return 'Target port must be 1–65535.'
 if(!value.flag_pattern||value.flag_pattern.length>4096)return 'Enter a fixed-width ASCII flag pattern (up to 4096 characters).'
 return ''
}

export function forwardingDestination(service:{upstream_address:string;forwarding?:unknown}):string {
 const config=service.forwarding as Record<string,unknown>|null|undefined
 return typeof config?.target_address==='string'?config.target_address:service.upstream_address
}
