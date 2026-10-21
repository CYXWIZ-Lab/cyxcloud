# Datasets from the CyxWiz Engine (DataStreamService)

How a CyxWiz account puts files into CyxCloud and makes them datasets a
training node can stream (TOFIX164 slice 1).

## Who the caller is

The Engine signs in to the CyxWiz API and presents that JWT to the gateway
(`authorization: Bearer <token>`). Its `sub` is the API's user id (a Mongo
ObjectId); the gateway maps it to one stable UUID (`auth::user_uuid_for_subject`:
UUID v5 in a fixed CyxWiz namespace; a UUID subject passes through unchanged) and
creates the `users` row on first sight. The same account therefore owns the same
buckets and datasets whether it arrives from the website, the Engine or a node.

## Which calls need a JWT

gRPC authentication is a tower layer (`grpc_auth.rs`) that reads the real request
path. Only two methods run without a JWT:

| Method | Why |
| --- | --- |
| `cyxcloud.node.NodeService/RegisterNode` | a storage node's first call |
| `cyxcloud.data.DataService/StreamData` | its credential is the dataset access token in the request body, which the handler checks (see `stream_file.md`) |

Everything else fails with `UNAUTHENTICATED` without a valid Bearer token. There
is no client-side way to opt out (the former `grpc-method` header is gone).

## Upload, then make a dataset

1. `DataStreamService.UploadFile` (client stream): the first message carries
   `UploadFileMetadata` (`filename` - one plain file name, `size_bytes`,
   optional `content_type`, optional 32-byte BLAKE3 `content_hash`); every
   following message carries a `chunk`. The gateway accepts the file only when
   the bytes add up to exactly `size_bytes` (and match the hash when given); at
   most 256 MB. The file lands in the caller's own bucket `u-<uuid>` (created on
   demand) and the response carries its `file_id`.
   - `UNAVAILABLE` "no storage node is online to hold the file": start a
     `cyxcloud-node`; the gateway stores shards on nodes, not on itself.
   - `FAILED_PRECONDITION` when the gateway runs `--memory-only`: memory objects
     have no file record and cannot become datasets.
2. `DataStreamService.CreateDataset` with those `file_ids` gives the dataset.
3. `DataStreamService.CreateAccessToken` (scope `stream`) gives the token a
   training node presents to `DataService.StreamData`.

`DataStreamService.DeleteDataset` removes a dataset the caller owns (its
`dataset_files`, tokens and shares go with it by cascade); the files stay in the
bucket.

## Protocol copies

The Engine compiles verbatim copies of `datastream.proto` and `data.proto`
(package names kept); keep them identical to the files here.
