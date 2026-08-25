import Link from "next/link";
import { Logo } from "@/components/logo";

const paths = [
  {
    href: "/docs/users/quickstart",
    eyebrow: "Using OpenWeights",
    title: "Get started",
    body: "Run the stack, mint an API key, and prove a byte-identical round-trip with the hf CLI you already have.",
  },
  {
    href: "/docs/developers/architecture",
    eyebrow: "How it works",
    title: "Read the internals",
    body: "The three services, the Xet protocol surface, the signed-URL contract, and every configuration value.",
  },
];

export default function HomePage() {
  return (
    <main className="flex flex-1 flex-col items-center justify-center px-4 py-20">
      <div className="w-full max-w-3xl">
        <Logo className="size-9 mb-8 text-fd-foreground" />

        <h1 className="font-mono text-3xl sm:text-4xl font-semibold tracking-tighter text-fd-foreground text-balance">
          A Hugging Face-compatible model hub, backed by Sia.
        </h1>

        <p className="mt-5 text-fd-muted-foreground text-lg leading-relaxed text-balance">
          Point the standard{" "}
          <code className="font-mono text-fd-foreground">hf</code> CLI at your
          own OpenWeights deployment. The bytes you upload are stored on the Sia
          network, and they come back byte-identical.
        </p>

        <div className="mt-10 grid gap-3 sm:grid-cols-2">
          {paths.map((p) => (
            <Link
              key={p.href}
              href={p.href}
              className="group rounded-xl border border-fd-border bg-fd-card p-5 transition-colors hover:bg-fd-accent"
            >
              <div className="font-mono text-xs uppercase tracking-widest text-fd-muted-foreground">
                {p.eyebrow}
              </div>
              <div className="mt-2 font-medium text-fd-card-foreground">
                {p.title}
              </div>
              <p className="mt-1.5 text-sm text-fd-muted-foreground leading-relaxed">
                {p.body}
              </p>
            </Link>
          ))}
        </div>

        <pre className="mt-10 overflow-x-auto rounded-xl border border-fd-border bg-fd-card p-5 font-mono text-sm leading-relaxed text-fd-card-foreground">
          <code>
            {"HF_TOKEN=<your-key> HF_ENDPOINT=http://localhost:8080 \\\n"}
            {"  hf upload <owner>/<repo> ./model-dir"}
          </code>
        </pre>
      </div>
    </main>
  );
}
