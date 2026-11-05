import {expect,test} from 'bun:test'
import {forwardingDraft,validateForwarding} from './forwarding'
test('editing a resolved forwarding endpoint preserves its target port and format',()=>{
 const draft=forwardingDraft({team_id:'two',target_address:'[::1]:9000',flag_pattern:'FLAG[A-Z0-9]{8}',framing:{type:'http1'}})
 expect(draft?.target_port).toBe(9000);expect(validateForwarding(draft)).toBe('');expect(validateForwarding(null)).toBe('')
 expect(validateForwarding({...draft!,target_port:0})).toContain('port')
})

test('editing mirroring framing does not mutate the saved service before Save',()=>{
 const original={team_id:'two',target_address:'127.0.0.1:9000',flag_pattern:'FLAG[A-Z]{8}',framing:{type:'length_prefixed' as const,offset:0,width:4,little_endian:false,length_includes_header:false}}
 const draft=forwardingDraft(original)!;if(draft.framing.type==='length_prefixed')draft.framing.offset=8
 expect(original.framing.offset).toBe(0)
})
