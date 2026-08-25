import { DocsLayout } from "fumadocs-ui/layouts/docs";
import { baseOptions } from "@/lib/layout.shared";
import { source } from "@/lib/source";

export default function Layout({ children }: LayoutProps<"/docs">) {
  return (
    <DocsLayout
      tree={source.getPageTree()}
      {...baseOptions()}
      sidebar={{ defaultOpenLevel: 1 }}
      tabs={[
        {
          title: "Using OpenWeights",
          description: "Run the stack and move models",
          url: "/docs/users",
        },
        {
          title: "How it works",
          description: "Internals, protocol, and reference",
          url: "/docs/developers",
        },
      ]}
    >
      {children}
    </DocsLayout>
  );
}
