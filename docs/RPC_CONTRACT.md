# PeerCastStation JSON-RPC 2.0 Contract Inventory

## 1. Protocol & Transport Specification

- **Specification Standard**: JSON-RPC 2.0 (RFC 4627 / JSON-RPC Specification 2010-03-26).
- **Transport Endpoint**: `POST /api/1`
- **Preamble Discovery**: `GET /api/1` (returns `getVersionInfo`)
- **API Description**: `GET /api/1/openrpc.json` (OpenRPC 1.2.6 descriptor)
- **Client Library**: `GET /api/1/peercaststation.js` (JavaScript RPC proxy generator)

### Security & Framing Controls (REQ-064, REQ-065)

| Control | Value / Requirement | Behavior on Violation |
|---|---|---|
| CSRF Header | `X-Requested-With: XMLHttpRequest` (or any non-empty value) | HTTP 400 Bad Request |
| Content-Length | Valid integer header required | HTTP 411 Length Required |
| Request Body Limit | 65,536 bytes (64 KiB) | HTTP 413 Payload Too Large |
| Timeout Limit | 5,000 milliseconds (5 seconds) | HTTP 408 Request Timeout |
| Content-Type | `application/json` | Parse error (-32700) if malformed |

### Access Control Categories (Station Parity: `OutputStreamType`)

1. **PlaybackAccess (`Play`)**: Allowed for general viewers and embedded player widgets.
   - Permits: `getVersionInfo`, `getAuthToken`, `getStatus`, `getChannels`, `getChannelStatus`, `getChannelInfo`.
2. **AdminAccess (`Interface`)**: Requires administrative authorization when listener authorization is enabled.
   - Permits: All Playback methods plus administrative queries and mutations (`getSettings`, `setSettings`, `setChannelInfo`, `stopChannel`, `bumpChannel`, `addListener`, `removeListener`, etc.).

### Authentication Modes

1. `Authorization: Basic <base64(user:pass)>`
2. `GET /api/1?auth=<base64(user:pass)>`
3. Cookie: `auth=<base64(user:pass)>`

When listener authorization is enabled and credentials are invalid or missing for `AdminAccess`:
- Returns JSON-RPC error code `-32001` (`Unauthorized`).

---

## 2. JSON-RPC 2.0 Standard & Extended Error Codes

| Code | Constant | Meaning |
|---|---|---|
| -32700 | `PARSE_ERROR` | Invalid JSON was received by the server. |
| -32600 | `INVALID_REQUEST` | The JSON sent is not a valid Request object. |
| -32601 | `METHOD_NOT_FOUND` | The method does not exist or is not available. |
| -32602 | `INVALID_PARAMS` | Invalid method parameter(s). |
| -32603 | `INTERNAL_ERROR` | Internal JSON-RPC error. |
| -32001 | `UNAUTHORIZED` | Client lacks administrative authorization for this method. |
| -32004 | `UNSUPPORTED_METHOD` | Method is deferred/unsupported; returns capability details (NO silent success stubs!). |

---

## 3. Audited UI Call Sites & Method Inventory

Audited from upstream `C:\Github\_upstream_peercaststation\PeerCastStation\PeerCastStation.UI.HTTP\APIHost.cs` and `html/js/*.js`:

### 3.1 Supported Core Methods (Milestone P37 Verified)

#### `getVersionInfo`
- **Access**: Playback / Admin
- **UI Call Sites**: `html/js/index.js:5`, `html/js/update.js:28`
- **Params**: None
- **Result**:
  ```json
  {
    "agentName": "PeerCastStation/1.0",
    "apiVersion": "1.0.0",
    "jsonrpc": "2.0"
  }
  ```

#### `getStatus`
- **Access**: Playback / Admin
- **UI Call Sites**: `html/js/index.js:15`, `html/js/play.js:10`, `html/js/relays.js:14`
- **Params**: None
- **Result**:
  ```json
  {
    "uptime": 120,
    "isFirewalled": false,
    "globalRelayEndPoint": ["203.0.113.10", 7144],
    "globalDirectEndPoint": ["203.0.113.10", 7144],
    "localRelayEndPoint": ["127.0.0.1", 7144],
    "localDirectEndPoint": ["127.0.0.1", 7144]
  }
  ```

#### `getChannels`
- **Access**: Playback / Admin
- **UI Call Sites**: `html/js/channels.js:12`, `html/js/play.js:25`, `html/js/relays.js:30`
- **Params**: None
- **Result**: Array of Channel objects.
  ```json
  [
    {
      "channelId": "0123456789ABCDEF0123456789ABCDEF",
      "name": "LiveStream",
      "status": "Receiving",
      "bitrate": 1024,
      "mimeType": "video/x-flv",
      "totalRelays": 5,
      "totalDirects": 12
    }
  ]
  ```

#### `getSettings`
- **Access**: Admin only
- **UI Call Sites**: `html/js/settings.js:40`
- **Params**: None
- **Result**:
  ```json
  {
    "maxRelays": 16,
    "maxRelaysPerChannel": 4,
    "maxDirects": 8,
    "maxDirectsPerChannel": 2,
    "maxUpstreamRate": 10000,
    "maxUpstreamRateIPv6": 10000
  }
  ```

#### `setSettings`
- **Access**: Admin only (Mutation)
- **UI Call Sites**: `html/js/settings.js:95`
- **Params**: Object with optional fields to update:
  ```json
  {
    "maxRelays": 20,
    "maxDirects": 10,
    "maxUpstreamRate": 15000
  }
  ```
- **Result**: `true` on success.
- **Errors**: Invalid parameter types return `-32602` (`INVALID_PARAMS`).

#### `getAuthToken`
- **Access**: Playback / Admin
- **UI Call Sites**: `html/js/channels.js:8`, `html/js/play.js:5`, `html/js/relays.js:8`
- **Params**: None
- **Result**: String token or `null`.

#### `getPlugins`
- **Access**: Admin only
- **UI Call Sites**: `html/js/index.js:35`
- **Params**: None
- **Result**: Array of plugin descriptions.

---

### 3.2 Deferred Methods (Explicit Capability Reporting; No Success Stubs)

When invoked, deferred methods return error `-32004` (`UNSUPPORTED_METHOD`) with capability payload:
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "error": {
    "code": -32004,
    "message": "Method 'broadcastChannel' is not supported in the current milestone",
    "data": {
      "method": "broadcastChannel",
      "status": "deferred",
      "capability": "ingest_manager"
    }
  }
}
```

Audited deferred method list:
1. `getExternalIPAddresses` (`settings.js`)
2. `getLogSettings` (`logs.js`)
3. `setLogSettings` (`logs.js`)
4. `getLog` (`logs.js`)
5. `clearLog` (`logs.js`)
6. `getChannelStatus` (`channels.js`)
7. `getChannelInfo` (`channels.js`)
8. `setChannelInfo` (`relays.js`)
9. `stopChannel` (`relays.js`)
10. `bumpChannel` (`relays.js`)
11. `getChannelOutputs` (`APIHost.cs:542`)
12. `stopChannelOutput` (`APIHost.cs:555`)
13. `getChannelConnections` (`relays.js`)
14. `stopChannelConnection` (`relays.js`)
15. `restartChannelConnection` (`relays.js`)
16. `getChannelRelayTree` (`relays.js`)
17. `getContentReaders` (`relays.js`)
18. `getSourceStreams` (`relays.js`)
19. `getYellowPageProtocols` (`settings.js`)
20. `getYellowPages` (`relays.js`, `settings.js`)
21. `addYellowPage` (`settings.js`)
22. `removeYellowPage` (`settings.js`)
23. `stopAnnounce` (`relays.js`)
24. `restartAnnounce` (`relays.js`)
25. `getListeners` (`relays.js`, `settings.js`)
26. `addListener` (`settings.js`)
27. `resetListenerAuthenticationKey` (`settings.js`)
28. `removeListener` (`settings.js`)
29. `setListenerAccepts` (`settings.js`)
30. `setListenerAuthorizationRequired` (`settings.js`)
31. `broadcastChannel` (`relays.js`)
32. `getBroadcastHistory` (`relays.js`)
33. `addBroadcastHistory` (`relays.js`)
34. `getNotificationMessages` (`common.js`)
35. `checkBandwidth` (`settings.js`)
36. `checkPorts` (`settings.js`)
37. `checkUpdate` (`APIHost.cs:1185`)
38. `updateAndRestart` (`update.js`)
39. `getUpdateStatus` (`update.js`)
40. `getNewVersions` (`common.js`, `update.js`)
41. `getYPChannels` (`APIHost.cs:1294`)
42. `updateYPChannels` (`channels.js`)
43. `setUserConfig` (`common.js`)
44. `getUserConfig` (`common.js`)

---

## 4. Batch and Notification Semantics

- **Notifications**: Any request lacking an `id` property or having `id: null` is treated as a notification. The server executes it, but **returns NO response** (per JSON-RPC 2.0 §4.1).
- **Batches**:
  - Request with JSON array `[req1, req2]` returns JSON array `[res1, res2]`.
  - Notifications within a batch omit their individual response from the output array.
  - If a batch consists entirely of notifications, server returns empty output (HTTP 204 No Content).
  - Empty batch array `[]` returns error `-32600` (`INVALID_REQUEST`).
