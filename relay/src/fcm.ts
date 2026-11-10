export type PushMessage = { provider?: "fcm" | "apns"; token: string; kind: "approval"|"completed"|"error"|"idle"; language: "fr"|"en"; workspace?: string };
export function notificationPayload(message: PushMessage) {
  const fr=message.language==="fr";
  const body=message.kind==="approval"?(fr?"Une autorisation ou une réponse est nécessaire.":"An approval or answer is needed."):message.kind==="completed"?(fr?"Votre agent a terminé.":"Your agent has finished."):message.kind==="idle"?(fr?"Aucune activité depuis une heure sur une autre machine : voyez s'il faut ramener ou arrêter ce qui y tourne.":"No activity for an hour on another machine: see whether to bring back or stop what runs there."):(fr?"Votre agent a rencontré une erreur.":"Your agent encountered an error.");
  return {message:{token:message.token,notification:{title:message.workspace||"Chauffeur",body},data:{chauffeur:"update"},android:{priority:message.kind==="approval"?"high":"normal",ttl:"300s",collapse_key:`chauffeur-${message.kind}`,notification:{channel_id:message.kind==="approval"?"approvals":"activity",tag:`chauffeur-${message.kind}`,default_sound:message.kind==="approval"}}}};
}
export function fcmSender(project: string, send: typeof fetch = fetch) {
  if(!/^[a-z][a-z0-9-]{4,62}$/.test(project))throw Error("Invalid Firebase project");
  let cached: {token:string;expires:number}|undefined;
  return async (message: PushMessage): Promise<"ok"|"unregistered"> => {
    if(!cached || cached.expires<Date.now()) {
      const response=await send("http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token",{headers:{"Metadata-Flavor":"Google"},signal:AbortSignal.timeout(3000)});
      if(!response.ok)throw Error("FCM identity unavailable");
      const value=await response.json() as {access_token:string;expires_in:number};
      if(typeof value.access_token!=="string" || !Number.isFinite(value.expires_in))throw Error("FCM identity unavailable");
      cached={token:value.access_token,expires:Date.now()+Math.max(0,value.expires_in-60)*1000};
    }
    const response=await send(`https://fcm.googleapis.com/v1/projects/${project}/messages:send`,{method:"POST",headers:{Authorization:`Bearer ${cached.token}`,"Content-Type":"application/json"},body:JSON.stringify(notificationPayload(message)),signal:AbortSignal.timeout(5000)});
    if(response.ok)return "ok";
    if(response.status===401)cached=undefined;
    const error=await response.json().catch(()=>({})) as {error?:{details?:{errorCode?:string}[]}};
    if(error.error?.details?.some(d=>d.errorCode==="UNREGISTERED"))return "unregistered";
    throw Error("FCM delivery failed");
  };
}
