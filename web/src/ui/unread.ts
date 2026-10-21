import { snap } from '../store';
export const unreadCount = (): number => snap.value?.todo.length ?? 0;
