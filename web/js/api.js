// The client of the API of section 5. All URLs are relative: the browser resolves them against <base href> (5.2).
// An error body is flat (5.4): {"error": code, "message": text, "retryable": bool, ...members}.
// A write has exactly one precondition header (5.5). A document keeps the etag of its read.

import { notify } from "./ui.js";

// An error of the API, or of the network (status 0, code "network_error").
export class ApiError extends Error {
  constructor({ status, code, message, retryable = false, extra = {}, requestId = null }) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
    this.retryable = retryable;
    this.extra = extra;
    this.requestId = requestId;
  }
}

export function isAbort(error) {
  return !!error && (error.name === "AbortError" || error.code === "aborted");
}

const hooks = { onUnauthorized: null };

// main.js sets the hook that shows the page of a 401.
export function setUnauthorizedHandler(fn) {
  hooks.onUnauthorized = fn;
}

export const enc = encodeURIComponent;

export function query(params) {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params || {})) if (value !== undefined && value !== null && value !== "") search.set(key, String(value));
  const text = search.toString();
  return text ? `?${text}` : "";
}

async function readError(response) {
  const requestId = response.headers.get("X-Request-Id");
  let body = null;
  try {
    body = await response.json();
  } catch (error) {
    body = null;
  }
  if (!body || typeof body !== "object" || typeof body.error !== "string") {
    return new ApiError({
      status: response.status,
      code: response.status >= 500 ? "internal_error" : "bad_request",
      message: `The server answered with status ${response.status}.`,
      retryable: response.status === 503,
      requestId,
    });
  }
  const { error: code, message, retryable, ...extra } = body;
  return new ApiError({
    status: response.status,
    code,
    message: typeof message === "string" ? message : `The server answered with status ${response.status}.`,
    retryable: retryable === true,
    extra,
    requestId,
  });
}

const stripEtag = (value) => (value || "").replace(/^W\//, "").replace(/"/g, "");

// One request. The answer is the response when `raw` is true, else the JSON body with the member `etag` set from the header ETag
// when the body has no `etag` of its own. `body` is a value that becomes JSON.
export async function request(method, path, { body, signal, headers = {}, raw = false } = {}) {
  const init = { method, signal, headers: { Accept: "application/json", ...headers }, credentials: "same-origin" };
  if (body !== undefined) {
    init.headers["Content-Type"] = "application/json";
    init.body = typeof body === "string" ? body : JSON.stringify(body);
  }
  let response;
  try {
    response = await fetch(path, init);
  } catch (error) {
    if (isAbort(error)) throw new ApiError({ status: 0, code: "aborted", message: "The request was cancelled." });
    throw new ApiError({ status: 0, code: "network_error", message: "Pupitre does not answer.", retryable: true });
  }
  if (!response.ok) {
    const error = await readError(response);
    if (response.status === 401 && hooks.onUnauthorized) hooks.onUnauthorized(error);
    if (error.code === "pdp_unavailable") notify({ level: "error", key: "pdp", text: "The access decision service does not answer. Try again later." });
    throw error;
  }
  if (raw) return response;
  let data = null;
  try {
    data = await response.json();
  } catch (error) {
    if (isAbort(error)) throw new ApiError({ status: 0, code: "aborted", message: "The request was cancelled." });
    throw new ApiError({ status: response.status, code: "bad_answer", message: "The answer is not valid JSON." });
  }
  const etag = stripEtag(response.headers.get("ETag"));
  if (etag && data && typeof data === "object" && !Array.isArray(data) && data.etag === undefined) {
    Object.defineProperty(data, "_etag", { value: etag, enumerable: false });
  }
  return data;
}

// The text of a document as stored, with its etag. The page diffs these bytes (5.13).
export async function getText(path, { signal } = {}) {
  const response = await request("GET", path, { signal, raw: true });
  return { text: await response.text(), etag: stripEtag(response.headers.get("ETag")) };
}

// A document and its etag. A missing document gives null.
export async function getDoc(path, { signal } = {}) {
  try {
    const { text, etag } = await getText(path, { signal });
    return { doc: JSON.parse(text), etag, text };
  } catch (error) {
    if (error instanceof ApiError && error.status === 404) return null;
    throw error;
  }
}

// A write of a document (5.5). `etag` null makes a create (If-None-Match: *), else a save (If-Match). The answer is {etag, version}.
export function putDoc(path, doc, { etag, message = "", signal } = {}) {
  const headers = etag ? { "If-Match": `"${etag}"` } : { "If-None-Match": "*" };
  const body = message ? { document: doc, message } : { document: doc };
  return request("PUT", path, { body, headers, signal });
}

export function deleteDoc(path, etag, { signal } = {}) {
  return request("DELETE", path, { headers: { "If-Match": `"${etag}"` }, signal });
}

// All the pages of a list route. The function stops at `max` items (a guard) and says if it cut the list.
export async function listAll(path, params = {}, { signal, max = 100000 } = {}) {
  const items = [];
  let after = null;
  for (let guard = 0; guard < 1000; guard += 1) {
    const page = await request("GET", `${path}${query({ ...params, after })}`, { signal });
    items.push(...(page.items || []));
    if (!page.next || items.length >= max) return { items, more: !!page.next };
    after = page.next;
  }
  return { items, more: true };
}

// Routes of the specification. Each id is escaped for the URL.
export const paths = {
  session: () => "api/session",
  catalog: () => "api/catalog",
  environments: () => "api/environments",
  environment: (e) => `api/environments/${enc(e)}`,
  common: (e) => `api/environments/${enc(e)}/common`,
  hosts: (e) => `api/environments/${enc(e)}/hosts`,
  host: (e, h) => `api/environments/${enc(e)}/hosts/${enc(h)}`,
  units: (e) => `api/environments/${enc(e)}/units`,
  unit: (e, u) => `api/environments/${enc(e)}/units/${enc(u)}`,
  values: (e, u) => `api/environments/${enc(e)}/units/${enc(u)}/values`,
  hostValues: (e, u, h) => `api/environments/${enc(e)}/units/${enc(u)}/hosts/${enc(h)}/values`,
  effective: (e, u) => `api/environments/${enc(e)}/units/${enc(u)}/effective`,
  secrets: (e, u = null) => (u === null ? `api/environments/${enc(e)}/secrets` : `api/environments/${enc(e)}/units/${enc(u)}/secrets`),
  secret: (e, u, name) => `${paths.secrets(e, u)}/${enc(name)}`,
  runs: () => "api/runs",
  run: (r) => `api/runs/${enc(r)}`,
  events: (r) => `api/runs/${enc(r)}/events`,
  output: (r) => `api/runs/${enc(r)}/output`,
  cancel: (r) => `api/runs/${enc(r)}/cancel`,
};

export const session = (signal) => request("GET", paths.session(), { signal });
export const catalog = (signal) => request("GET", paths.catalog(), { signal });

export const node = {
  environment: (e) => `environment:${e}`,
  unit: (e, u) => `environment:${e}/unit:${u}`,
  host: (e, u, h) => `environment:${e}/unit:${u}/host:${h}`,
};
