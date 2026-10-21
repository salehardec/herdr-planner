/* Modal-ish dialogs the Esc key must be able to close: add a galaxy, add a universe, the todo pop-over, the Station,
   the ship's computer. The arrival notice is not modal: it closes with its own buttons, not with Esc. */
import { signal } from '@preact/signals';
import { closeStation, shipDlg, stationDlg } from './ui/station-api';

export const addDialogs = {
  galaxy: signal(false),
  universe: signal<{ galaxy: string } | null>(null),
  firstRun: signal(false),
  popover: signal<{ id: string; repo: string; galaxy?: string } | null>(null),
};

export const dialogOpen = (): boolean =>
  addDialogs.galaxy.value || !!addDialogs.universe.value || addDialogs.firstRun.value || !!addDialogs.popover.value
  || !!stationDlg.value || shipDlg.value;

export function closeDialogs(): void {
  if (addDialogs.popover.value) { addDialogs.popover.value = null; return; }
  if (stationDlg.value) { closeStation(); return; }
  if (shipDlg.value) { shipDlg.value = false; return; }
  if (addDialogs.universe.value) { addDialogs.universe.value = null; return; }
  if (addDialogs.galaxy.value) { addDialogs.galaxy.value = false; return; }
  if (addDialogs.firstRun.value) { addDialogs.firstRun.value = false; }
}
