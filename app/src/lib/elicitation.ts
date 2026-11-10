import type { JsonValue } from "@chauffeur/protocol";

export function jsonObject(value: JsonValue | undefined): Record<string, JsonValue | undefined> | null {
  return value && typeof value === "object" && !Array.isArray(value) ? value : null;
}

/** Simple fields get native controls; richer JSON Schemas keep a JSON editor. */
export function elicitationFields(schema: JsonValue): Array<[string, Record<string, JsonValue | undefined>]> | null {
  const properties = jsonObject(jsonObject(schema)?.properties);
  if (!properties) return null;
  const fields: Array<[string, Record<string, JsonValue | undefined>]> = [];
  for (const [name, value] of Object.entries(properties)) {
    const field = jsonObject(value);
    if (!field || !["string", "boolean", "number", "integer"].includes(String(field.type))) return null;
    fields.push([name, field]);
  }
  return fields;
}

/** Invalid JSON stays in the editor, never sent as a string pretending to be form content. */
export function parseElicitationJson(text: string): { content: JsonValue; error: string | null } {
  try {
    const content: JsonValue = JSON.parse(text);
    if (!jsonObject(content)) return { content: null, error: "Enter a JSON object." };
    return { content, error: null };
  } catch {
    return { content: null, error: "Enter valid JSON." };
  }
}
