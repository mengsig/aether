import { useApp, useReread } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { ModelPicker } from "@aether-ui/ui";
import { records, ReadNote } from "../memory/shared.tsx";

export function useModels(conn: string, screen: string) {
  const models = useReread(conn, `${screen}:models`, { cmd: "list_models" }, ["models"]);
  const auth = useReread(conn, `${screen}:auth`, { cmd: "auth_status" }, ["auth"]);
  return { models: records<aether.ModelInfo>(models?.value), auth: records<aether.AuthInfo>(auth?.value), modelsRead: models, authRead: auth };
}

export function Models({ conn, screen, value, onChange, testId, defaultLabel, disabled }: {
  conn: string; screen: string; value: string; onChange: (value: string) => void; testId: string; defaultLabel?: string; disabled?: boolean;
}) {
  const app = useApp();
  const data = useModels(conn, screen);
  return <div className="b3-stack">
    <ModelPicker models={data.models} auth={data.auth} value={value} onChange={onChange} data-testid={testId} defaultLabel={defaultLabel} disabled={disabled} />
    <ReadNote entry={data.modelsRead} retry={() => app.reread(conn, `${screen}:models`, { cmd: "list_models" }, ["models"])} />
    {data.authRead?.error ? <ReadNote entry={data.authRead} retry={() => app.reread(conn, `${screen}:auth`, { cmd: "auth_status" }, ["auth"])} /> : null}
  </div>;
}
