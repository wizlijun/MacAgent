import { defineWorkersConfig } from "@cloudflare/vitest-pool-workers/config";

export default defineWorkersConfig({
  test: {
    poolOptions: {
      workers: {
        singleFork: true,
        wrangler: { configPath: "./wrangler.toml" },
        // DOs are in-memory in tests; avoids SQLite WAL file isolation issues
        // with vitest-pool-workers when using new_sqlite_classes in wrangler.toml
        miniflare: {
            unsafeEphemeralDurableObjects: true,
            bindings: {
              CF_CALLS_KEY_ID: "test-key",
              CF_CALLS_KEY_API_TOKEN: "test-token",
              // Dummy P-256 PKCS8 key for testing ES256 JWT signing (generated via openssl)
              // Real APNs key is never committed; actual ES256 delivery verified in M4.7 真机环节
              APNS_AUTH_KEY: `-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgsTWO/SM8akWlyCmI
8XWTB7kY6tqH+M1192//al7oZv6hRANCAARxm4mDAfLuF0c+nZ13nNMa0iy08N+M
+J+VSWHqELONLF5bI5lUrUGI4rtqsIqqtdOSQ3MLCFF/qn3INaEinulL
-----END PRIVATE KEY-----`,
              APNS_KEY_ID: "TESTKEYID1",
              APNS_TEAM_ID: "TESTTEAMID",
              APNS_BUNDLE_ID: "com.example.testapp",
              APNS_ENV: "sandbox",
            },
          },
      },
    },
  },
});
