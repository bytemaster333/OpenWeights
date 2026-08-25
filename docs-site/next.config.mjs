import { createMDX } from "fumadocs-mdx/next";

const withMDX = createMDX();

// On GitHub Pages the site is served under a repo subpath (e.g. /OpenWeights),
// so the build sets PAGES_BASE_PATH to that prefix. Local dev and custom-domain
// deploys leave it unset and serve from the root.
const basePath = process.env.PAGES_BASE_PATH || "";

/** @type {import('next').NextConfig} */
const config = {
  output: "export",
  reactStrictMode: true,
  basePath,
  images: { unoptimized: true },
};

export default withMDX(config);
