import test from 'node:test';
import assert from 'node:assert/strict';
import { runAgentTurn } from '../src/agent-flow.js';

test('continues from weather data to writing the file', async () => {
  const calls = [];
  const plans = [
    { id: 'weather', message: 'Checking weather', actions: [{ label: 'Weather' }] },
    { id: 'write', message: 'Updated test.txt with current weather.', actions: [{ label: 'Write file' }] },
  ];
  const outcome = await runAgentTurn({
    prompt: 'replace test.txt with the weather in Pomona',
    history: [],
    ask: async (prompt, history) => {
      calls.push({ prompt, history: structuredClone(history) });
      return plans.shift();
    },
    execute: async (plan) => plan.id === 'weather'
      ? [{ action: 'Weather', detail: 'Pomona', status: 'success', output: '20.8 °C, humidity 57%' }]
      : [{ action: 'Write file', detail: 'test.txt', status: 'success', output: 'Wrote 28 bytes' }],
  });
  assert.equal(calls.length, 2);
  assert.match(calls[1].history.at(-1).content, /20\.8 °C/);
  assert.equal(outcome.results[1].action, 'Write file');
  assert.match(outcome.message, /Updated test.txt/);
});

test('does not claim completion when the next model call fails', async () => {
  let calls = 0;
  const outcome = await runAgentTurn({
    prompt: 'update a file with the weather',
    history: [],
    ask: async () => {
      calls += 1;
      if (calls === 1) return { id: 'weather', message: 'Checking weather', actions: [{}] };
      throw new Error('empty response');
    },
    execute: async () => [{ action: 'Weather', detail: 'Pomona', status: 'success', output: '20 °C' }],
  });
  assert.equal(outcome.results.length, 1);
  assert.match(outcome.message, /could not continue/);
});

test('keeps going if the model stops after fetching weather without updating the file', async () => {
  let calls = 0;
  const outcome = await runAgentTurn({
    prompt: 'replace that with the current weather of Pomona',
    history: [{ role: 'assistant', content: 'Write file test.txt: success' }],
    ask: async () => {
      calls += 1;
      if (calls === 1) return { id: 'weather', message: 'Fetching weather', actions: [{}] };
      if (calls === 2) return { id: 'premature', message: 'Done', actions: [] };
      return { id: 'write', message: 'Writing file', actions: [{}] };
    },
    execute: async (plan) => plan.id === 'weather'
      ? [{ action: 'Weather', detail: 'Pomona', status: 'success', output: '20.8 °C' }]
      : [{ action: 'Write file', detail: 'test.txt', status: 'success', output: 'Wrote 7 bytes' }],
  });
  assert.equal(calls, 3);
  assert.equal(outcome.results[1].action, 'Write file');
});
