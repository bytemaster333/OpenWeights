import { defineConfig } from "astro/config"
import starlight from "@astrojs/starlight"

export default defineConfig({
  site: "https://docs.siahub.app",
  integrations: [
    starlight({
      title: "siahub docs",
      description: "xet-compatible model hub on sia",
      sidebar: [
        {
          label: "start here",
          items: [
            { label: "what is siahub", slug: "index" },
            { label: "quickstart", slug: "guides/quickstart" },
          ],
        },
        {
          label: "guides",
          items: [
            { label: "upload a model", slug: "guides/upload" },
            { label: "download a model", slug: "guides/download" },
            { label: "self-host", slug: "guides/self-host" },
            { label: "mirror on huggingface", slug: "guides/hf-bridge" },
          ],
        },
        {
          label: "reference",
          items: [
            { label: "api", slug: "reference/api" },
            { label: "cli env vars", slug: "reference/env" },
            { label: "architecture", slug: "reference/architecture" },
          ],
        },
      ],
    }),
  ],
})
