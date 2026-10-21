# Streaming a file to a training node (DataService.StreamData)

`DataService.StreamData` (gRPC, package `cyxcloud.data`) sends one stored file
to a CyxWiz server node, in order, in chunks. Since TOFIX164 it is truthful:

- **A token is required.** `StreamDataRequest.access_token` holds a dataset
  access token from `DataStreamService.CreateAccessToken`. The gateway checks it
  (hash lookup, not revoked, not expired), that it carries the `stream` or
  `read` scope, and that the token's dataset contains the requested file.
  Without it the call fails with `UNAUTHENTICATED`; a token for another dataset
  fails with `PERMISSION_DENIED`.
- **No silent skips.** A chunk with no storage location, or one no storage node
  can serve, ends the stream with `DATA_LOSS` and a message naming the chunk
  ("chunk 3 of 7 could not be served: ..."). Nothing is marked complete.
- **The bytes are checked twice.** The gateway hashes what it assembled with
  BLAKE3 and compares with the file's stored content hash before the last
  chunk; a mismatch is `DATA_LOSS`. The last chunk (`is_last = true`) carries
  `stream_sha256` (hex) and `total_bytes` for the SHA-256 of every streamed byte
  in order, which the node verifies before it uses the file.
- `shuffle` is refused (`INVALID_ARGUMENT`): a file stream is ordered.
  `batch_size` (KB) only sets how many bytes travel per message.

The node side of this is cyxwiz-server-node's `cloud://` dataset source
(Engine repo, TOFIX164 slice 3).
