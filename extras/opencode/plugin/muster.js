// OpenCode calls every function this module exports as a plugin, so it exports only `Muster`.
//
// Tells the Muster daemon that owns this pane what OpenCode is doing: working, waiting on you or
// idle, its model and how full its context is, and the session's id. It runs only
// `"$MUSTER_DAEMON" report`, and does nothing outside a Muster pane.
//
// What each event means was measured on OpenCode 1.18.34 (docs/observations/opencode-1.18.34.md,
// section 5): `session.status` busy as a turn starts, `session.idle` as every turn ends, Esc and a
// refused permission included, `permission.asked` and `permission.replied` around a permission
// prompt, and token counts on each assistant message.

import { execFile } from "node:child_process";

// What OpenCode's events have said so far, which decides what the next one means.
function initial() {
  return {
    // Sessions a sub-agent runs in: their turns start and end inside the main session's, and
    // reporting them would read the pane idle while its agent works.
    children: new Set(),
    permissions: new Set(),
    state: null,
    // The cost of each assistant message, by id, since each is reported as it grows.
    costs: new Map(),
    // The last usage reported: OpenCode updates a message several times with the same counts.
    usage: null,
  };
}

// The reports one event calls for, each a list of `report`'s arguments; `window` gives a model's
// context window in tokens, when known.
function reports(event, seen, window = () => undefined) {
  const properties = event.properties ?? {};
  const info = properties.info ?? {};
  const session = properties.sessionID ?? info.sessionID ?? info.id;
  if ((event.type === "session.created" || event.type === "session.updated") && info.parentID) {
    seen.children.add(info.id);
  }
  if (session !== undefined && seen.children.has(session)) {
    return [];
  }
  const state = (said) => {
    if (seen.state === said) {
      return [];
    }
    seen.state = said;
    return [["--agent", "opencode", "--state", said]];
  };
  switch (event.type) {
    case "session.created":
      seen.state = null;
      seen.permissions.clear();
      seen.costs.clear();
      return [["--clear"], ["--agent", "opencode", "--session-id", info.id]];
    case "session.status":
      if (properties.status?.type === "idle" || seen.permissions.size > 0) {
        return [];
      }
      return state("working");
    case "permission.asked":
      seen.permissions.add(properties.id);
      return state("blocked");
    case "permission.replied":
      seen.permissions.delete(properties.requestID);
      return seen.permissions.size > 0 ? [] : state("working");
    case "session.idle":
      seen.permissions.clear();
      return state("idle");
    case "message.updated":
      return usage(info, seen, window);
    default:
      return [];
  }
}

function usage(message, seen, window) {
  const total = message.tokens?.total;
  if (message.role !== "assistant" || !total) {
    return [];
  }
  const model = `${message.providerID}/${message.modelID}`;
  const said = ["--model", model];
  const limit = window(message.providerID, message.modelID);
  if (limit > 0) {
    const used = Math.min(100, (total * 100) / limit);
    said.unshift("--context-used", used.toFixed(2));
  }
  if (typeof message.cost === "number") {
    seen.costs.set(message.id, message.cost);
    const spent = [...seen.costs.values()].reduce((sum, cost) => sum + cost, 0);
    said.push("--cost-usd", spent.toFixed(4));
  }
  const key = said.join(" ");
  if (seen.usage === key) {
    return [];
  }
  seen.usage = key;
  return [said];
}

// The context window of each model, from OpenCode's own provider list, asked once.
async function windows(client) {
  const found = new Map();
  try {
    const answer = await client.config.providers();
    for (const provider of answer?.data?.providers ?? []) {
      for (const [id, model] of Object.entries(provider.models ?? {})) {
        found.set(`${provider.id}/${id}`, model.limit?.context);
      }
    }
  } catch {
    // Without it the model is still reported, and context is not.
  }
  return found;
}

export const Muster = async ({ client }) => {
  const daemon = process.env.MUSTER_DAEMON;
  if (!daemon) {
    return {};
  }
  const seen = initial();
  let limits = null;
  let sending = Promise.resolve();
  const send = (args) =>
    new Promise((done) => execFile(daemon, ["report", ...args], { timeout: 5000 }, () => done()));
  return {
    event: async ({ event }) => {
      if (event.type === "message.updated" && limits === null) {
        limits = await windows(client);
      }
      const window = (provider, model) => limits?.get(`${provider}/${model}`);
      for (const args of reports(event, seen, window)) {
        // One at a time, in the events' order: a working report must not land after the idle
        // that followed it.
        sending = sending.then(() => send(args));
      }
      await sending;
    },
  };
};
