import { defineWorkersConfig } from "@cloudflare/vitest-pool-workers/config";

export default defineWorkersConfig({
  test: {
    poolOptions: {
      workers: {
        singleFork: true,
        wrangler: { configPath: "./wrangler.toml" },
      },
    },
  },
});
