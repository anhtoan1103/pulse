import { Badge } from "@/components/ui";

export function IncidentStatusBadge({
  incident,
}: {
  incident: { resolved_at: string | null; recovered_at: string | null };
}) {
  if (incident.resolved_at) return <Badge tone="neutral">resolved</Badge>;
  if (incident.recovered_at) return <Badge tone="good">recovered</Badge>;
  return <Badge tone="bad">open</Badge>;
}
