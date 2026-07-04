import { defineConfig } from "astro/config"
import starlight from "@astrojs/starlight"

export default defineConfig({
  site: "https://docs.openweights.app",
  integrations: [
    starlight({
      title: "OpenWeights Docs",
      description: "Xet-compatible model hub on Sia",
      favicon: "/favicon.svg",
      sidebar: [
        {
          label: "Start here",
          items: [
            { label: "What is OpenWeights", slug: "index" },
            { label: "Quickstart", slug: "guides/quickstart" },
          ],
        },
        {
          label: "Guides",
          items: [
            { label: "Upload a model", slug: "guides/upload" },
            { label: "Download a model", slug: "guides/download" },
            { label: "Self-host", slug: "guides/self-host" },
            { label: "Mirror on Hugging Face", slug: "guides/hf-bridge" },
          ],
        },
        {
          label: "Reference",
          items: [
            { label: "API", slug: "reference/api" },
            { label: "CLI env vars", slug: "reference/env" },
            { label: "Architecture", slug: "reference/architecture" },
          ],
        },
      ],
    }),
  ],
})
