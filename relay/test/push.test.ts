import { expect,test } from "bun:test";
import { notificationPayload, fcmSender } from "../src/fcm";
import { pushKind, PushObserver } from "../src/push-observer";
import { startRelay } from "../src/server";
import type { EventEnvelope, RemoteDevice, PushPreferences } from "@chauffeur/protocol";
import type { DaemonConnection } from "../src/daemon";
const preferences:PushPreferences={enabled:true,approvals:true,completed:true,errors:true,idle_reminders:true,show_workspace:false,language:"fr"};
function event(type:string,extra:object={}):EventEnvelope{return {seq:1,ts:new Date().toISOString(),event:{scope:"chat",workspace_id:"w",chat_id:"c",event:{type,...extra} as never}};}
function device():RemoteDevice{return {info:{id:"a".repeat(22),peer_id:"peer",name:"phone",relay:"ws://127.0.0.1:41240",created_at:"now"},host_token:"h".repeat(43),phone_token:"p".repeat(43),secret:"s".repeat(43),push:{token:"fcm_token_123456789",preferences:{...preferences}}};}
test("only actionable live events create push kinds; no interrupted or text events",()=>{
 expect(pushKind(event("permission_request"))).toBe("approval");expect(pushKind(event("question"))).toBe("approval");
 expect(pushKind(event("turn_ended",{outcome:"success"}))).toBe("completed");expect(pushKind(event("turn_ended",{outcome:"error"}))).toBe("error");
 expect(pushKind(event("turn_ended",{outcome:"interrupted"}))).toBeNull();expect(pushKind({...event("question"),seq:null})).toBeNull();expect(pushKind(event("assistant_text",{text:"private"}))).toBeNull();
});
test("FCM payload has only approved labels and optional workspace name",()=>{
 const payload=notificationPayload({token:"fcm_token_123456789",kind:"approval",language:"fr",workspace:"My workspace"});
 expect(payload.message.notification.title).toBe("My workspace");expect(payload.message.notification.body).toContain("autorisation");expect(payload.message.android.notification.channel_id).toBe("approvals");expect(payload.message.data).toEqual({chauffeur:"update"});
 expect(notificationPayload({token:"token",kind:"completed",language:"en"}).message.notification.title).toBe("Chauffeur");
});
test("observer respects preferences, foreground, revocation, and does not replay history",async()=>{
 let devices=[device()];let connected=true;let calls:{method:string;params:any}[]=[];let sends:any[]=[];
 const daemon:DaemonConnection={onEvent:()=>{},onClose:()=>{},close(){},async call(method,params){calls.push({method,params});if(method==="events.subscribe")return {subscription:"s"};if(method==="workspace.get")return {title:"Private workspace"};return {};}};
 const observer=new PushObserver(daemon,()=>devices,()=>connected,(async(_url:unknown,options?:RequestInit)=>{sends.push(JSON.parse(String(options?.body)));return new Response("ok");}) as unknown as typeof fetch);
 await observer.start();expect(calls[0].params.after_seq).toBeUndefined();
 const notify=()=>daemon.onEvent({params:{envelope:event("permission_request",{input:{command:"secret command"}})}});
 notify();await Bun.sleep(20);expect(sends).toHaveLength(0);
 connected=false;notify();await Bun.sleep(20);expect(sends).toHaveLength(1);expect(sends[0].workspace).toBeUndefined();expect(JSON.stringify(sends)).not.toContain("secret command");expect(calls.some(c=>c.method==="workspace.get")).toBe(false);
 notify();await Bun.sleep(20);expect(sends).toHaveLength(1);
 devices=[];daemon.onEvent({params:{envelope:event("turn_ended",{outcome:"success"})}});await Bun.sleep(20);expect(sends).toHaveLength(1);observer.stop();
});
test("push endpoint requires host role and strips unknown fields before sending",async()=>{
 const sent:any[]=[];const relay=startRelay({adminToken:"a".repeat(64),port:0,push:async value=>{sent.push(value);return "ok";}});
 const url=`http://127.0.0.1:${relay.server.port}`;const id="d".repeat(22),hostToken="h".repeat(43),phoneToken="p".repeat(43);
 try {
 await fetch(`${url}/v1/devices`,{method:"POST",headers:{Authorization:`Bearer ${"a".repeat(64)}`},body:JSON.stringify({deviceId:id,hostToken,phoneToken})});
 const send=(token:string,body:object)=>fetch(`${url}/v1/devices/${id}/push`,{method:"POST",headers:{Authorization:`Bearer ${token}`},body:JSON.stringify(body)});
 const body={token:"fcm_token_123456789",kind:"approval",language:"fr",workspace:"Workspace",text:"secret command"};
 expect((await send(phoneToken,body)).status).toBe(401);expect((await send(hostToken,{...body,token:"bad"})).status).toBe(400);
 expect((await send(hostToken,body)).status).toBe(200);expect(sent).toHaveLength(1);expect(sent[0].text).toBeUndefined();expect((await send(hostToken,body)).status).toBe(429);
 }finally{relay.stop();}
});
test("disabled notifications and category preferences prevent delivery", async () => {
  const phone = device();
  const pref = phone.push!.preferences;
  const sends: unknown[] = [];
  const calls: string[] = [];
  const daemon: DaemonConnection = {
    onEvent() {}, onClose() {}, close() {},
    async call(method) {
      calls.push(method);
      return method === "events.subscribe" ? {subscription:"test"} : {};
    },
  };
  const observer = new PushObserver(daemon, () => [phone], () => false,
    (async (_url: unknown, options?: RequestInit) => {
      sends.push(JSON.parse(String(options?.body)));
      return new Response("ok");
    }) as unknown as typeof fetch);
  try {
    await observer.start();
    pref.enabled = false;
    daemon.onEvent({params:{envelope:event("question")}});
    await Bun.sleep(20);
    expect(sends).toHaveLength(0);
    pref.enabled = true;
    pref.approvals = pref.completed = pref.errors = false;
    for (const envelope of [event("question"), event("turn_ended", {outcome:"success"}), event("turn_ended", {outcome:"error"})]) {
      daemon.onEvent({params:{envelope}});
    }
    await Bun.sleep(20);
    expect(sends).toHaveLength(0);
    expect(calls).not.toContain("remote.check");
    pref.errors = true;
    daemon.onEvent({params:{envelope:event("turn_ended", {outcome:"error"})}});
    await Bun.sleep(20);
    expect(sends).toHaveLength(1);
    expect(sends[0]).toMatchObject({kind:"error", language:"fr"});
  } finally { observer.stop(); }
});
test("workspace names are included only when allowed and removed on later events", async () => {
  const phone = device();
  const sends: Record<string, unknown>[] = [];
  let workspaceReads = 0;
  const daemon: DaemonConnection = {
    onEvent() {}, onClose() {}, close() {},
    async call(method) {
      if (method === "events.subscribe") return {subscription:"test"};
      if (method === "workspace.get") { workspaceReads++; return {title:"My workspace\n"}; }
      return {};
    },
  };
  const observer = new PushObserver(daemon, () => [phone], () => false,
    (async (_url: unknown, options?: RequestInit) => {
      sends.push(JSON.parse(String(options?.body)));
      return new Response("ok");
    }) as unknown as typeof fetch);
  try {
    await observer.start();
    phone.push!.preferences.show_workspace = true;
    daemon.onEvent({params:{envelope:event("question")}});
    await Bun.sleep(20);
    expect(sends[0].workspace).toBe("My workspace");
    expect(workspaceReads).toBe(1);
    phone.push!.preferences.show_workspace = false;
    daemon.onEvent({params:{envelope:event("turn_ended", {outcome:"success"})}});
    await Bun.sleep(20);
    expect(sends).toHaveLength(2);
    expect(sends[1].workspace).toBeUndefined();
    expect(workspaceReads).toBe(1);
  } finally { observer.stop(); }
});
test("unregistered FCM device is reported without leaking provider errors",async()=>{
 let count=0;const sender=fcmSender("acme-chauffeur",(async()=>++count===1?Response.json({access_token:"secret",expires_in:3600}):Response.json({error:{details:[{errorCode:"UNREGISTERED"}]}},{status:404})) as unknown as typeof fetch);
 expect(await sender({token:"token",kind:"error",language:"en"})).toBe("unregistered");
});
test("an idle reminder pushes once for all its workspaces, with their names when allowed",async()=>{
 const reminder:EventEnvelope={seq:7,ts:new Date().toISOString(),event:{scope:"registry",event:{type:"idle_reminder",workspace_ids:["w1","w2"]}}};
 expect(pushKind(reminder)).toBe("idle");expect(pushKind({...reminder,seq:null})).toBeNull();
 expect(pushKind({seq:8,ts:"now",event:{scope:"registry",event:{type:"chat_removed",workspace_id:"w",chat_id:"c"}}})).toBeNull();
 const phone=device();phone.push!.preferences.show_workspace=true;const sends:any[]=[];
 const daemon:DaemonConnection={onEvent:()=>{},onClose:()=>{},close(){},async call(method,params:any){if(method==="events.subscribe"){expect(params.registry).toBe(true);return {subscription:"s"};}if(method==="workspace.get")return {slug:params.workspace==="w1"?"alpha":"beta"};return {};}};
 const observer=new PushObserver(daemon,()=>[phone],()=>false,(async(_url:unknown,options?:RequestInit)=>{sends.push(JSON.parse(String(options?.body)));return new Response("ok");}) as unknown as typeof fetch);
 await observer.start();
 daemon.onEvent({params:{envelope:reminder}});await Bun.sleep(20);
 expect(sends).toHaveLength(1);expect(sends[0].kind).toBe("idle");expect(sends[0].workspace).toBe("alpha, beta");
 phone.push!.preferences.idle_reminders=false;daemon.onEvent({params:{envelope:{...reminder,seq:9,event:{scope:"registry",event:{type:"idle_reminder",workspace_ids:["w3"]}}}}});await Bun.sleep(20);
 expect(sends).toHaveLength(1);observer.stop();
 const payload=notificationPayload({token:"fcm_token_123456789",kind:"idle",language:"fr",workspace:"alpha, beta"});
 expect(payload.message.notification.body).toContain("Aucune activité depuis une heure");expect(payload.message.android.notification.channel_id).toBe("activity");expect(payload.message.android.notification.default_sound).toBe(false);
});
