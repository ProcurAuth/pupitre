// The catalog of capabilities (4.2, 5.15): the schemas, and with the runner the playbooks and the commands. It stays the same for the
// life of the server process, so the page reads it one time.

import * as api from "../api.js";
import { app } from "../config.js";

export async function loadCatalog(signal) {
  if (!app.catalog) app.catalog = await api.catalog(signal);
  return app.catalog;
}

export const schemaById = (catalog, id) => (catalog.schemas || []).find((s) => s.id === id) || null;
export const groupsOf = (catalog) => (catalog.schemas || []).map((s) => s.group);
export const playbooksOf = (catalog, kind) => (catalog.playbooks || []).filter((p) => p.kind === kind);
