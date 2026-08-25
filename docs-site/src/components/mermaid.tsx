"use client";

import { useTheme } from "next-themes";
import { use, useEffect, useId, useState } from "react";

export function Mermaid({ chart }: { chart: string }) {
  const [mounted, setMounted] = useState(false);

  useEffect(() => {
    setMounted(true);
  }, []);

  if (!mounted) return null;
  return <MermaidContent chart={chart} />;
}

const cache = new Map<string, Promise<unknown>>();

function cachePromise<T>(
  key: string,
  setPromise: () => Promise<T>,
): Promise<T> {
  const cached = cache.get(key);
  if (cached) return cached as Promise<T>;

  const promise = setPromise();
  cache.set(key, promise);
  return promise;
}

function MermaidContent({ chart }: { chart: string }) {
  const id = useId();
  const { resolvedTheme } = useTheme();
  const { default: mermaid } = use(
    cachePromise("mermaid", () => import("mermaid")),
  );

  const dark = resolvedTheme === "dark";

  mermaid.initialize({
    startOnLoad: false,
    securityLevel: "loose",
    fontFamily: "inherit",
    themeCSS: "margin: 0 auto;",
    theme: "base",
    // cool-neutral monochrome, mirroring the site palette so diagrams read as
    // part of the page rather than as mermaid's purple default.
    themeVariables: dark
      ? {
          background: "#1b2225",
          primaryColor: "#252d31",
          primaryTextColor: "#f5f8f8",
          primaryBorderColor: "#3b4448",
          secondaryColor: "#2c3538",
          tertiaryColor: "#202729",
          lineColor: "#7d8f94",
          textColor: "#f5f8f8",
          fontSize: "14px",
        }
      : {
          background: "#ffffff",
          primaryColor: "#f2f4f4",
          primaryTextColor: "#101416",
          primaryBorderColor: "#d3dadc",
          secondaryColor: "#e9edee",
          tertiaryColor: "#f7f9f9",
          lineColor: "#5f7176",
          textColor: "#101416",
          fontSize: "14px",
        },
  });

  const { svg, bindFunctions } = use(
    cachePromise(`${chart}-${resolvedTheme}`, () =>
      mermaid.render(id.replace(/:/g, ""), chart.replaceAll("\\n", "\n")),
    ),
  );

  return (
    <div
      className="my-6 overflow-x-auto rounded-xl border border-fd-border bg-fd-card p-5"
      ref={(container) => {
        if (container) bindFunctions?.(container);
      }}
      // biome-ignore lint/security/noDangerouslySetInnerHtml: mermaid returns rendered SVG
      dangerouslySetInnerHTML={{ __html: svg }}
    />
  );
}
