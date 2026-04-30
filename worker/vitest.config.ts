import { defineWorkersConfig } from "@cloudflare/vitest-pool-workers/config";

export default defineWorkersConfig({
  test: {
    poolOptions: {
      workers: {
        singleFork: true,
        wrangler: { configPath: "./wrangler.toml" },
        // DOs are in-memory in tests; avoids SQLite WAL file isolation issues
        // with vitest-pool-workers when using new_sqlite_classes in wrangler.toml
        miniflare: { unsafeEphemeralDurableObjects: true },
      },
    },
  },
});
