import Ajv from "ajv";
import Ajv2020 from "ajv/dist/2020.js";
import addFormats from "ajv-formats";
import type { ValidateFunction } from "ajv";
import { ErrorCode, type JsonValue, type RequestResponse } from "../../protocol.ts";
import { RpcError } from "../../rpc/peer.ts";
import { type PendingRequests } from "../pending.ts";
import type { CodexAlwaysAllowed } from "./always.ts";
import type { McpServerElicitationRequestParams } from "./generated/v2/McpServerElicitationRequestParams.ts";
import type { McpServerElicitationRequestResponse } from "./generated/v2/McpServerElicitationRequestResponse.ts";

function formValidator(schema: JsonValue): ValidateFunction {
  if (!schema || typeof schema !== "object" || Array.isArray(schema) || schema.type !== "object") {
    throw new Error("the form must describe a JSON object");
  }
  const options = { strict: true, strictTypes: false, logger: false as const };
  const ajv = schema.$schema === "https://json-schema.org/draft/2020-12/schema" ? new Ajv2020(options) : new Ajv(options);
  addFormats(ajv);
  ajv.addKeyword({ keyword: "enumNames", schemaType: "array", valid: true });
  const validate = ajv.compile(schema);
  if ("$async" in validate) throw new Error("asynchronous forms are not supported");
  return validate;
}

/** A form with no field: accepting it says yes and nothing else. */
function emptyForm(schema: JsonValue): boolean {
  if (!schema || typeof schema !== "object" || Array.isArray(schema)) return false;
  const properties = schema.properties;
  return properties === undefined || (typeof properties === "object" && properties !== null && Object.keys(properties).length === 0);
}

function safeUrl(value: string): boolean {
  try {
    const url = new URL(value);
    return ["http:", "https:"].includes(url.protocol) && !url.username && !url.password;
  } catch {
    return false;
  }
}

/**
 * Puts an MCP confirmation to the user. One with nothing to fill in can be accepted always: the
 * chat keeps its server and message as a rule and accepts it itself when it comes again (see
 * always.ts); any other is answered each time.
 */
export async function askMcpElicitation(
  params: McpServerElicitationRequestParams,
  requests: PendingRequests,
  always: CodexAlwaysAllowed,
  track: (requestId: string | null) => void,
): Promise<McpServerElicitationRequestResponse> {
  // Codex optional fields become omitted fields in Chauffeur’s JSON wire type.
  const schema: JsonValue = params.mode === "url" ? null : JSON.parse(JSON.stringify(params.requestedSchema));
  const url = params.mode === "url" ? params.url : null;
  let validate: ValidateFunction | undefined;
  let unsupported: string | null = null;
  try {
    if (schema !== null) validate = formValidator(schema);
    else if (!url || !safeUrl(url)) throw new Error("the confirmation URL must use HTTP or HTTPS without credentials");
  } catch (error) {
    unsupported = `Cannot accept ${params.serverName}'s confirmation: ${error instanceof Error ? error.message : "unsupported form"}. Decline or cancel it.`;
  }
  const blank = !unsupported && validate !== undefined && emptyForm(schema) && validate({});
  const rules = blank ? [`${params.serverName}: ${params.message}`] : [];
  if (always.covers(rules)) return { action: "accept", content: {}, _meta: null };
  try {
    const outcome = await requests.ask("elicitation", (requestId) => {
      track(requestId);
      return {
        type: "mcp_elicitation", request_id: requestId, server_name: params.serverName, message: params.message, schema, url, error: unsupported,
        ...(rules[0] ? { rule: rules[0] } : {}),
      };
    }, {
      subject: { toolName: params.serverName, input: {}, rules },
      resolution: (o) => (rules.length > 0 && o.type === "elicitation" && o.action === "accept" && o.always ? { rules } : {}),
      validate(response: RequestResponse) {
        if (response.type !== "elicitation") throw new RpcError(ErrorCode.BAD_REQUEST, "answer this MCP confirmation with accept, decline or cancel");
        if (response.always && (response.action !== "accept" || rules.length === 0)) {
          throw new RpcError(ErrorCode.BAD_REQUEST, `${params.serverName}'s confirmation is answered each time: send it without always`);
        }
        if (response.action !== "accept") return;
        if (unsupported) throw new RpcError(ErrorCode.BAD_REQUEST, unsupported);
        if (validate && !validate(response.content)) {
          const error = validate.errors?.[0];
          throw new RpcError(ErrorCode.BAD_REQUEST, `Invalid MCP response ${error?.instancePath || "/"}: ${error?.message ?? "check the form"}`);
        }
        if (url !== null && response.content !== null) throw new RpcError(ErrorCode.BAD_REQUEST, "URL confirmations do not accept form content");
      },
    });
    if (outcome.type !== "elicitation") return { action: "cancel", content: null, _meta: null };
    if (rules.length > 0 && outcome.action === "accept" && outcome.always) {
      always.add(rules);
      requests.allowCovered((call) => always.covering(call.rules), true);
    }
    return { action: outcome.action, content: outcome.action === "accept" ? outcome.content : null, _meta: null };
  } finally {
    track(null);
  }
}
