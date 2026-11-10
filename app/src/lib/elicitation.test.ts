import { expect, test } from "bun:test";
import { elicitationFields, parseElicitationJson } from "./elicitation";

test("primitive fields use controls while richer schemas keep JSON editing", () => {
  expect(elicitationFields({ type: "object", properties: { approved: { type: "boolean" } } })?.[0]?.[0]).toBe("approved");
  expect(elicitationFields({ type: "object", properties: {} })).toEqual([]);
  expect(elicitationFields({ type: "object", properties: { choices: { type: "array", items: { type: "string" } } } })).toBeNull();
});

test("JSON responses preserve booleans and reject malformed or non-object input", () => {
  expect(parseElicitationJson('{"approved":false}')).toEqual({ content: { approved: false }, error: null });
  for (const text of ["invalid", "null", "true", "[]"]) expect(parseElicitationJson(text).error).not.toBeNull();
});
