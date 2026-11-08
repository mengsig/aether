import { useApp, useReread } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { ModelPicker } from "@aether-ui/ui";
import { records, ReadNote } from "../memory/shared.tsx";

export function useModels(conn: string, screen: string) {
  const models = useReread(conn, `${screen}:models`, { cmd: "list_models" }, ["models"]);
  const providers = useReread(conn, `${screen}:providers`, { cmd: "providers" }, ["providers"]);
  return { models: records<aether.ModelInfo>(models?.value), providers: records<aether.ProviderInfo>(providers?.value), modelsRead: models, providersRead: providers };
}

export function Models({ conn, screen, value, onChange, testId, defaultLabel, disabled }: {
  conn: string; screen: string; value: string; onChange: (value: string) => void; testId: string; defaultLabel?: string; disabled?: boolean;
}) {
  const app = useApp();
  const data = useModels(conn, screen);
  return <div className="b3-stack">
    <ModelPicker models={data.models} providers={data.providers} value={value} onChange={onChange} data-testid={testId} defaultLabel={defaultLabel} disabled={disabled} />
    <ReadNote entry={data.modelsRead} retry={() => app.reread(conn, `${screen}:models`, { cmd: "list_models" }, ["models"])} />
    {data.providersRead?.error ? <ReadNote entry={data.providersRead} retry={() => app.reread(conn, `${screen}:providers`, { cmd: "providers" }, ["providers"])} /> : null}
  </div>;
}
