function port(value, label, defaultPort) {
  const raw = value ?? String(defaultPort);
  const number = Number(raw);
  if (!/^\d+$/.test(raw) || !Number.isInteger(number) || number < 1 || number > 65535) {
    throw new Error(label + " must be a TCP port between 1 and 65535.");
  }
  return number;
}

function enabled(value) {
  return value === "1" || value?.toLowerCase() === "true";
}

function httpsUrl(value, label) {
  let url;
  try { url = new URL(value); }
  catch { throw new Error(label + " must be an HTTPS URL."); }
  if (url.protocol !== "https:" || url.username || url.password || url.hash || url.search) {
    throw new Error(label + " must be an HTTPS URL without embedded credentials, query or fragment.");
  }
  return url;
}

export function configFromEnvironment(env, mode) {
  const apiPort = port(env.OPENTALLY_PORT ?? env.VOUCHER_PORT, "OPENTALLY_PORT", 3000);
  const writes = enabled(env.OPENTALLY_MCP_WRITES);
  // A book with users needs an OpenTally API token (Authorization: Bearer).
  const token = env.OPENTALLY_API_TOKEN?.trim();
  if (token !== undefined && token !== "" && !/^ot_[0-9a-f]{64}$/.test(token)) {
    throw new Error("OPENTALLY_API_TOKEN must be an OpenTally API token starting with ot_.");
  }
  const auth = token ? { token } : {};
  if (mode === "stdio") return { apiPort, writes, ...auth };
  const listenPort = port(env.OPENTALLY_MCP_PORT, "OPENTALLY_MCP_PORT", 8787);
  const resource = httpsUrl(env.OPENTALLY_MCP_RESOURCE_URL, "OPENTALLY_MCP_RESOURCE_URL");
  if (resource.pathname !== "/mcp") throw new Error("OPENTALLY_MCP_RESOURCE_URL must end in /mcp.");
  httpsUrl(env.OPENTALLY_MCP_ISSUER, "OPENTALLY_MCP_ISSUER");
  const introspection = httpsUrl(env.OPENTALLY_MCP_INTROSPECTION_URL, "OPENTALLY_MCP_INTROSPECTION_URL");
  if (!env.OPENTALLY_MCP_CLIENT_ID || !env.OPENTALLY_MCP_CLIENT_SECRET) {
    throw new Error("The HTTP connector requires the OAuth introspection client ID and secret.");
  }
  const approvedOrigins = (env.OPENTALLY_MCP_ALLOWED_ORIGINS ?? "https://chatgpt.com,https://claude.ai")
    .split(",").map((entry) => entry.trim()).filter(Boolean);
  const origins = new Set([resource.origin, ...approvedOrigins.map((origin) =>
    httpsUrl(origin, "OPENTALLY_MCP_ALLOWED_ORIGINS").origin)]);
  return {
    ...auth, apiPort, writes, listenPort, origins, resource: resource.href, issuer: env.OPENTALLY_MCP_ISSUER,
    introspection: introspection.href, clientId: env.OPENTALLY_MCP_CLIENT_ID,
    clientSecret: env.OPENTALLY_MCP_CLIENT_SECRET,
  };
}
