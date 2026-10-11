import fs from 'node:fs';
export default function(pi) {
 pi.on('ui_prompt_start',async()=>{fs.appendFileSync(process.env.PROBE_LOG,JSON.stringify({event:'fixture_start_delayed',pid:process.pid})+'\n');await new Promise(r=>setTimeout(r,1500));});
 pi.on('ui_prompt_end',()=>{fs.appendFileSync(process.env.PROBE_LOG,JSON.stringify({event:'fixture_end_received',pid:process.pid})+'\n');});
 pi.registerCommand('probe-auto-ui',{handler:async(_,ctx)=>{await ctx.ui.confirm('Temporary prompt','Harmless timeout',{timeout:100});fs.appendFileSync(process.env.PROBE_LOG,JSON.stringify({event:'fixture_ui_resolved',pid:process.pid})+'\n');}});
}
