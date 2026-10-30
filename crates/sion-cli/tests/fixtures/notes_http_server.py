"""An MCP server over Streamable HTTP on the official Python SDK (mcp 2.x),
protected with OAuth by the SDK's own authorization server framework: for
checking sion's client against an implementation it did not write.

The authorization server keeps clients, codes and tokens in memory and
approves every authorization at once, as a person who always says yes
would: a real one shows a consent page first. Access tokens last
`--token-seconds` (default 3600), so a short life shows renewal.

Usage: notes_http_server.py PORT [--token-seconds N] [--sse]
"""

import secrets
import sys
import time

from mcp.server.auth.provider import (
    AccessToken,
    AuthorizationCode,
    AuthorizationParams,
    OAuthAuthorizationServerProvider,
    RefreshToken,
    construct_redirect_uri,
)
from mcp.server.auth.settings import AuthSettings, ClientRegistrationOptions, RevocationOptions
from mcp.server.mcpserver import MCPServer
from mcp.shared.auth import OAuthClientInformationFull, OAuthToken

PORT = int(sys.argv[1])
TOKEN_SECONDS = int(sys.argv[sys.argv.index("--token-seconds") + 1]) if "--token-seconds" in sys.argv else 3600
SSE = "--sse" in sys.argv
BASE = f"http://127.0.0.1:{PORT}"


class Provider(OAuthAuthorizationServerProvider):
    def __init__(self):
        self.clients = {}
        self.codes = {}
        self.access = {}
        self.refresh = {}

    async def get_client(self, client_id):
        return self.clients.get(client_id)

    async def register_client(self, client_info: OAuthClientInformationFull):
        self.clients[client_info.client_id] = client_info

    async def authorize(self, client, params: AuthorizationParams):
        code = secrets.token_urlsafe(24)
        self.codes[code] = AuthorizationCode(
            code=code,
            scopes=params.scopes or ["notes:read"],
            expires_at=time.time() + 300,
            client_id=client.client_id,
            code_challenge=params.code_challenge,
            redirect_uri=params.redirect_uri,
            redirect_uri_provided_explicitly=params.redirect_uri_provided_explicitly,
            resource=params.resource,
            subject="ana",
        )
        return construct_redirect_uri(str(params.redirect_uri), code=code, state=params.state)

    async def load_authorization_code(self, client, authorization_code):
        return self.codes.get(authorization_code)

    def _issue(self, client_id, scopes, resource):
        access = "at_" + secrets.token_urlsafe(24)
        refresh = "rt_" + secrets.token_urlsafe(24)
        expires = int(time.time()) + TOKEN_SECONDS
        self.access[access] = AccessToken(
            token=access, client_id=client_id, scopes=scopes, expires_at=expires, resource=resource, subject="ana"
        )
        self.refresh[refresh] = RefreshToken(
            token=refresh, client_id=client_id, scopes=scopes, resource=resource, subject="ana"
        )
        return OAuthToken(
            access_token=access,
            token_type="Bearer",
            expires_in=TOKEN_SECONDS,
            refresh_token=refresh,
            scope=" ".join(scopes),
        )

    async def exchange_authorization_code(self, client, authorization_code):
        self.codes.pop(authorization_code.code, None)
        return self._issue(client.client_id, authorization_code.scopes, authorization_code.resource)

    async def load_refresh_token(self, client, refresh_token):
        return self.refresh.get(refresh_token)

    async def exchange_refresh_token(self, client, refresh_token, scopes):
        self.refresh.pop(refresh_token.token, None)
        return self._issue(client.client_id, scopes or refresh_token.scopes, refresh_token.resource)

    async def load_access_token(self, token):
        found = self.access.get(token)
        if found and found.expires_at and found.expires_at < time.time():
            return None
        return found

    async def revoke_token(self, token):
        self.access.pop(token.token, None)
        self.refresh.pop(token.token, None)


notes = {"groceries": "oat milk, lentils, coffee"}

server = MCPServer(
    name="notes",
    version="1.0.0",
    instructions="Notes kept on the server.",
    auth_server_provider=Provider(),
    auth=AuthSettings(
        issuer_url=BASE,
        # The protected resource is the endpoint a client is given: the
        # stream's, on the old transport.
        resource_server_url=f"{BASE}/sse" if SSE else f"{BASE}/mcp",
        client_registration_options=ClientRegistrationOptions(
            enabled=True, valid_scopes=["notes:read", "notes:write"], default_scopes=["notes:read"]
        ),
        revocation_options=RevocationOptions(enabled=True),
        required_scopes=["notes:read"],
        validate_token_resource=True,
    ),
)


@server.tool(annotations={"readOnlyHint": True, "openWorldHint": False})
def read_note(name: str) -> str:
    """Read a note by its name."""
    return notes.get(name, f"there is no note {name}")


@server.tool(annotations={"readOnlyHint": False, "openWorldHint": False, "idempotentHint": True})
def write_note(name: str, text: str) -> str:
    """Write a note under a name, replacing any of that name."""
    notes[name] = text
    return f"wrote {name}"


if SSE:
    server.run(transport="sse", host="127.0.0.1", port=PORT)
else:
    server.run(transport="streamable-http", host="127.0.0.1", port=PORT)
