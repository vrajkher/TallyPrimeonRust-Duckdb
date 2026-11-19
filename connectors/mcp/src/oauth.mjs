import { scopes } from "./server.mjs";

export function metadataUrl(config) {
  return new URL("/.well-known/oauth-protected-resource/mcp", config.resource).href;
}

export function protectedResourceMetadata(config) {
  return {
    resource: config.resource,
    resource_name: "OpenTally",
    authorization_servers: [config.issuer],
    bearer_methods_supported: ["header"],
    scopes_supported: config.writes ? [scopes.read, scopes.write] : [scopes.read],
  };
}

function challenge(config, scope, error, message) {
  return {
    ok: false, message,
    challenge: 'Bearer resource_metadata="' + metadataUrl(config) + '", scope="' + scope
      + '", error="' + error + '", error_description="' + message + '"',
  };
}

const formEncode = (value) => new URLSearchParams({ p: value }).toString().slice(2);

export function createAuthorizer(config, check = fetch) {
  const basic = "Basic " + Buffer.from(formEncode(config.clientId) + ":" + formEncode(config.clientSecret)).toString("base64");
  return async (authorization, scope) => {
    const token = /^Bearer ([A-Za-z0-9._~+\/-]+={0,2})$/i.exec(authorization ?? "")?.[1];
    if (!token || token.length > 16384) {
      return challenge(config, scope, "invalid_token", "Sign in to OpenTally.");
    }
    let claims;
    try {
      const response = await check(config.introspection, {
        method: "POST", redirect: "error",
        headers: { Authorization: basic, Accept: "application/json" },
        body: new URLSearchParams({ token, token_type_hint: "access_token" }),
        signal: AbortSignal.timeout(8_000),
      });
      if (!response.ok) return { ok: false, message: "The OpenTally authorization service is unavailable." };
      claims = await response.json();
    } catch {
      return { ok: false, message: "The OpenTally authorization service is unavailable." };
    }
    if (claims?.active !== true || (claims.iss !== undefined && claims.iss !== config.issuer)
        || (claims.exp !== undefined && (!Number.isFinite(claims.exp) || claims.exp <= Date.now() / 1000))
        || (claims.nbf !== undefined && (!Number.isFinite(claims.nbf) || claims.nbf > Date.now() / 1000))) {
      return challenge(config, scope, "invalid_token", "Sign in again to OpenTally.");
    }
    const audience = Array.isArray(claims.aud) ? claims.aud : [claims.aud];
    if (!audience.includes(config.resource)) {
      return challenge(config, scope, "invalid_token", "This access token is for a different resource.");
    }
    const granted = typeof claims.scope === "string" ? claims.scope.split(/\s+/) : [];
    if (!granted.includes(scope)) {
      return challenge(config, scope, "insufficient_scope", "Authorize the required OpenTally permission.");
    }
    return { ok: true };
  };
}
