import {api} from '/assets/api.js';
import {el,table,button} from '/assets/view.js';
export async function render(page){
 const settings=await api('/system/settings');
 page.replaceChildren(el('h1','Settings'),el('p','Current installation settings. Checker credentials and signing keys stay on the server. Operator configuration changes take effect after Core restart.'));
 page.append(table(['Setting','Value'],[['Flag format',settings.flag_pattern||'Not configured'],['Automatic captured submission',String(settings.automatic_submission)],['Checker',settings.checker_adapter||'Not configured'],['Checker batch size',settings.checker_batch_size],['Sandbox configured',String(settings.sandbox_configured)+' (runs validate prerequisites)'],['Worker concurrency',settings.worker_concurrency],['Worker networks',settings.supported_worker_networks.join(', ')],['Access token lifetime',settings.access_lifetime_seconds+' seconds'],['Absolute refresh lifetime',settings.refresh_absolute_lifetime_seconds+' seconds']]));
 page.append(button('Export service and filter configuration',async()=>{const value=await api('/config/export');const url=URL.createObjectURL(new Blob([JSON.stringify(value,null,2)],{type:'application/json'}));const link=el('a');link.href=url;link.download='hydra-config.json';link.click();setTimeout(()=>URL.revokeObjectURL(url),1000);}));
}
