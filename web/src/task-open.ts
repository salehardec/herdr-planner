/* What a click on a task key in a text does. The Galaxy flies to the task (main.ts); the phone's light page opens
   the task's sheet (mobile/main.tsx). The chip itself does not know the world, so a page without one can show it. */
import type { Task } from './types';

export const taskOpener: { open: (task: Task) => void } = { open: () => {} };
