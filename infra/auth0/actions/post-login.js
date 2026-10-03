/**
 * clipos post-login Action. Managed by infra/auth0: edit this file, not the dashboard.
 *
 * The tenant is shared, so this only acts on clipos applications (secret CLIENT_IDS);
 * logins to anything else pass through untouched.
 *
 * @param {Event} event
 * @param {PostLoginAPI} api
 */
exports.onExecutePostLogin = async (event, api) => {
  const cliposClients = (event.secrets.CLIENT_IDS || "").split(",").filter(Boolean);
  if (!cliposClients.includes(event.client.client_id)) {
    return;
  }

  // New apps are auto-enabled on every tenant connection; clipos is Google-only.
  if (event.connection.strategy !== "google-oauth2") {
    return api.access.deny("clipos only supports Sign in with Google.");
  }
  if (!event.user.email || !event.user.email_verified) {
    return api.access.deny("Your Google account's email address isn't verified.");
  }
  const email = event.user.email.toLowerCase();

  // Production logins must be on the invite list. The api enforces it too; this stops
  // strangers before they get a token. Dev logins skip it (Auth0 can't reach localhost).
  const prod =
    event.client.client_id === event.secrets.PROD_CLIENT_ID ||
    event.resource_server?.identifier === event.secrets.PROD_AUDIENCE;
  if (prod) {
    let allowed;
    try {
      allowed = await checkInvite(event.secrets, email);
    } catch (err) {
      console.log(`invite check failed: ${err}`);
      return api.access.deny("clipos couldn't check your invite just now. Try again in a minute.");
    }
    if (!allowed) {
      return api.access.deny(
        "This Google account isn't invited to clipos. Ask a friend who runs it for an invite.",
      );
    }
  }

  // Claims read by clipos-core (auth::Claims). Keep the namespace in sync.
  const ns = "https://clips.spawnpoint.run/";
  api.accessToken.setCustomClaim(`${ns}email`, email);
  if (event.user.name) {
    api.accessToken.setCustomClaim(`${ns}name`, event.user.name);
  }
  if (event.user.picture) {
    api.accessToken.setCustomClaim(`${ns}picture`, event.user.picture);
  }
};

/** POSTs the email to the api's invite check (not a query string: keeps it out of logs). */
async function checkInvite(secrets, email) {
  const res = await fetch(secrets.INVITE_CHECK_URL, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-clipos-internal-secret": secrets.INVITE_CHECK_SECRET,
    },
    body: JSON.stringify({ email }),
    signal: AbortSignal.timeout(5000),
  });
  if (!res.ok) {
    throw new Error(`status ${res.status}`);
  }
  const body = await res.json();
  return body.allowed === true;
}
