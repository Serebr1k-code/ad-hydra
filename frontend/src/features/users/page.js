import {api,clear} from '/assets/api.js';
import {el,button,table,detail,date,edit} from '/assets/view.js';
export async function render(page) {
 const [users,actor]=await Promise.all([api('/users'),api('/auth/me')]);
 page.replaceChildren(el('h1','Users and audit'),el('p','All active accounts have identical full privileges. Create or recover accounts through the local CLI. Password changes revoke every session of that account.'));
 const signOutSelf=id=>{if(id===actor.id){clear();window.dispatchEvent(new Event('hydra-session-changed'));return true;}return false;};
 page.append(table(['Username','Active','Created / updated','Actions'],users.map(u=>{
  const actions=el('div');
  actions.append(button('Edit account',()=>edit('Account',{username:u.username,active:u.active},async input=>{await api(`/users/${u.id}`,'PATCH',{...input,expected_revision:u.revision});if(!input.active&&signOutSelf(u.id))return;await render(page);})));
  const password=el('input');password.type='password';password.placeholder='New password (12+ bytes)';password.autocomplete='new-password';password.setAttribute('aria-label','New password for '+u.username);
  actions.append(password,button('Reset password',async()=>{if(!password.value)throw Error('Enter a new password');await api(`/users/${u.id}`,'PATCH',{expected_revision:u.revision,password:password.value});password.value='';if(!signOutSelf(u.id))await render(page);}));
  return[u.username,String(u.active),date(u.created_at)+' / '+date(u.updated_at),actions];
 })));
 const history=el('div');let cursor=0;
 const load=async(reset=false)=>{if(reset){cursor=0;history.replaceChildren();}const rows=await api('/audit?after='+cursor);if(rows.length){cursor=rows.at(-1).id;history.append(detail(rows));}else history.append(el('p','No more audit events.'));};
 page.append(button('Load audit',()=>load(true)),button('Next audit page',()=>load()),history);
}
