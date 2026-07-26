import { createMachine } from 'xstate';

export const scanMachine = createMachine({
    id: 'scan',
    initial: 'idle',
    states: {
        idle: {
            on: {
                START: 'scanning',
                CALIBRATE: 'calibrating',
            },
        },
        scanning: {
            on: {
                STOP: 'idle',
                COMPLETE: 'processing',
                ERROR: 'error',
            },
        },
        processing: {
            on: {
                FINISH: 'idle'
            }
        },
        calibrating: {
            on: {
                DONE: 'idle',
            },
        },
        error: {
            on: {
                RESET: 'idle',
            },
        },
    },
});
