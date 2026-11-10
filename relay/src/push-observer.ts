import type { EventEnvelope, RemoteDevice } from "@chauffeur/protocol";
import type { DaemonConnection } from "./daemon";

export type PushKind = "approval" | "completed" | "error" | "idle";
export function pushKind(envelope: EventEnvelope): PushKind | null {
  if (envelope.seq == null) return null;
  // Moved workspaces left an hour without activity, one reminder for all of them.
  if (envelope.event.scope === "registry") return envelope.event.event.type === "idle_reminder" ? "idle" : null;
  if (envelope.event.scope !== "chat") return null;
  const event = envelope.event.event;
  if (event.type === "permission_request" || event.type === "question") return "approval";
  if (event.type === "turn_ended") return event.outcome === "success" ? "completed" : event.outcome === "error" ? "error" : null;
  return null;
}
// Separate live subscription: pushes keep working when the phone's UI socket is closed.
// No historical replay, transcript, command or agent output is sent to FCM.
export class PushObserver {
  private stopped = false;
  private subscription?: string;
  private queue: EventEnvelope[] = [];
  private draining = false;
  private sent = new Map<string, number>();
  constructor(private daemon: DaemonConnection, private devices: () => RemoteDevice[], private connected: (id: string) => boolean, private send: typeof fetch = fetch) {}
  async start() {
    this.daemon.onEvent = message => {
      const envelope = (message as {params?: {envelope?: EventEnvelope}})?.params?.envelope;
      if (!envelope || !pushKind(envelope) || this.stopped) return;
      if (this.queue.length >= 64) this.queue.shift();
      this.queue.push(envelope); void this.drain();
    };
    const result = await this.daemon.call("events.subscribe", {all:true,registry:true,workspaces:[],chats:[],ephemeral:false}) as {subscription:string};
    this.subscription = result.subscription;
  }
  stop() { this.stopped = true; this.queue = []; if (this.subscription) void this.daemon.call("events.unsubscribe",{subscription:this.subscription}).catch(()=>{}); }
  private async drain() {
    if (this.draining) return;
    this.draining=true;
    try { while (!this.stopped && this.queue.length) { const event=this.queue.shift()!; try { await this.deliver(event); } catch { /* No payloads or tokens in logs. Next event can retry. */ } } }
    finally { this.draining=false; }
  }
  private async deliver(envelope: EventEnvelope) {
    const kind=pushKind(envelope);if(!kind)return;
    let workspace: string | undefined;
    const scope = envelope.event;
    const workspaces = scope.scope==="chat" ? [scope.workspace_id] : scope.scope==="registry" && scope.event.type==="idle_reminder" ? scope.event.workspace_ids : [];
    const subject = scope.scope==="chat" ? scope.chat_id : workspaces.join(",");
    for(const initial of this.devices()) {
      if(this.stopped)return;
      // Re-read the latest grant/preferences before delivery; never retain an old grant.
      const device=this.devices().find(d=>d.info.id===initial.info.id);
      const push=device?.push; const pref=push?.preferences;
      if(!device || !push || !pref?.enabled || this.connected(device.info.id) || !(kind==="approval"?pref.approvals:kind==="completed"?pref.completed:kind==="idle"?pref.idle_reminders:pref.errors))continue;
      const key=`${device.info.id}:${subject}:${kind}`;
      if(Date.now()-(this.sent.get(key)??0)<10_000)continue;
      await this.daemon.call("remote.check",{device_id:device.info.id});
      if(pref.show_workspace && workspace===undefined) {
        const names:string[]=[];
        for(const id of workspaces) {
          const value=await this.daemon.call("workspace.get",{workspace:id}) as {title?:string;slug?:string};
          names.push((value.title||value.slug||"").replace(/[\u0000-\u001f\u007f]/g,""));
        }
        workspace=names.filter(Boolean).join(", ").slice(0,80);
      }
      if(this.stopped || !this.devices().some(d=>d.info.id===device.info.id && d.push?.preferences.enabled))continue;
      const url=new URL(device.info.relay);url.protocol=url.protocol==="wss:"?"https:":"http:";url.pathname=`/v1/devices/${device.info.id}/push`;
      const response=await this.send(url,{method:"POST",headers:{Authorization:`Bearer ${device.host_token}`,"Content-Type":"application/json"},body:JSON.stringify({provider:push.provider ?? "fcm",token:push.token,kind,language:pref.language,workspace:pref.show_workspace?workspace:undefined}),signal:AbortSignal.timeout(8000)});
      if(response.status===410) { await this.daemon.call("remote.push_set",{device_id:device.info.id,config:{token:null,preferences:{...pref,enabled:false}}}); }
      if(response.ok)this.sent.set(key,Date.now());
      if(this.sent.size>256)this.sent.delete(this.sent.keys().next().value!);
    }
  }
}
