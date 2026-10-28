const MAX_ROUNDS = 4;

export async function runAgentTurn({ prompt, history, ask, execute, onPhase = () => {} }) {
  const context = [...history];
  const results = [];
  let nextPrompt = prompt;
  const fileContext = /\bfile\b|\b[\w-]+\.(txt|md|json|csv)\b/i.test(prompt)
    || history.some((item) => /Write file.*\.(txt|md|json|csv)/i.test(item.content));
  const expectsFileWrite = fileContext && /\b(create|write|save|replace|update|edit)\b/i.test(prompt);

  for (let round = 0; round < MAX_ROUNDS; round += 1) {
    onPhase('Thinking');
    let plan;
    try {
      plan = await ask(nextPrompt, context);
    } catch (error) {
      return { message: `I could not continue the request: ${error}`, results };
    }

    if (!plan.actions?.length && expectsFileWrite && !results.some((item) => item.action === 'Write file' || item.action === 'Run command')) {
      context.push({ role: 'user', content: nextPrompt });
      context.push({ role: 'assistant', content: plan.message || 'No action was taken.' });
      nextPrompt = `The original request is not complete: no file has been written. Use write_file to update the intended file with the actual data already returned by tools. Original request: ${prompt}`;
      continue;
    }
    if (!plan.actions?.length) {
      return { message: plan.message || (results.length ? 'Done.' : 'I have no actions to run.'), results };
    }

    onPhase('Working');
    let batch;
    try {
      batch = await execute(plan);
    } catch (error) {
      return { message: `I could not run the next action: ${error}`, results };
    }
    results.push(...batch);
    if (batch.some((item) => item.status !== 'success')) {
      return { message: 'I could not complete the request.', results };
    }
    if (expectsFileWrite && batch.some((item) => item.action === 'Write file')) {
      return { message: plan.message || 'The file has been updated.', results };
    }

    context.push({ role: 'user', content: nextPrompt });
    context.push({
      role: 'assistant',
      content: `${plan.message || ''}\nActual action results:\n${batch.map((item) => `${item.action} (${item.detail}): ${item.status}\n${item.output}`).join('\n')}`.slice(0, 8000),
    });
    nextPrompt = `Continue the original request: ${prompt}\nUse the actual action results above. Do not repeat completed actions. If the request was to replace or update a file with API data, write the returned data to that file now. Return an empty actions array only when the original request is complete.`;
  }

  return { message: 'I ran the actions shown, but stopped before I could verify that the full request was complete.', results };
}
