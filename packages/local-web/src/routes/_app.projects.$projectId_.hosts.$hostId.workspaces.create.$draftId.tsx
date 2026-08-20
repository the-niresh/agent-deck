import { createFileRoute } from '@tanstack/react-router';
import { LocalProjectKanban } from '@/pages/kanban/LocalProjectKanban';
import { projectSearchValidator } from '@agent-deck/web-core/project-search';

export const Route = createFileRoute(
  '/_app/projects/$projectId_/hosts/$hostId/workspaces/create/$draftId'
)({
  validateSearch: projectSearchValidator,
  component: LocalProjectKanban,
});
