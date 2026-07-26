export interface HistoryAction<T = unknown> {
    forward: T;
    inverse: T;
}

export const undoStack: HistoryAction[] = [];
export const redoStack: HistoryAction[] = [];

export const pushAction = (act: HistoryAction) => { undoStack.push(act); redoStack.length = 0; };
export const undo = (): unknown => {
    const a = undoStack.pop();
    if (a) {
        redoStack.push(a);
        return a.inverse;
    }
    return undefined;
};
export const redo = (): unknown => {
    const a = redoStack.pop();
    if (a) {
        undoStack.push(a);
        return a.forward;
    }
    return undefined;
};
