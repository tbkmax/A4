import Alpine from 'alpinejs';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import { openUrl } from '@tauri-apps/plugin-opener';
import { runAgentTurn } from './agent-flow.js';
import './style.css';

Alpine.data('agentApp', () => ({
  view: 'chat',
  skillsOpen: false,
  skills: [
    { icon: '▤', name: 'List files', description: 'See what is in your working folder' },
    { icon: '◫', name: 'Read files', description: 'Open text files for context' },
    { icon: '✎', name: 'Write files', description: 'Create or update text files' },
    { icon: '▣', name: 'Folders', description: 'Create folders when requested' },
    { icon: '⌫', name: 'Delete files', description: 'Remove a file in the workspace' },
    { icon: '⌘', name: 'Commands', description: 'Run a shell command in the workspace' },
    { icon: '☼', name: 'Weather', description: 'Current weather from Open-Meteo' },
    { icon: '✳', name: 'Cat facts', description: 'A fact from Cat Facts API' },
    { icon: '◇', name: 'GitHub profiles', description: 'Public profile information' },
  ],
  ollama: { running: false, models: [], error: '' },
  model: '',
  workspace: '',
  prompt: '',
  messages: [],
  busy: false,
  busyLabel: 'Thinking',
  notice: '',
  noticeTimer: null,

  async init() {
    const savedWorkspace = localStorage.getItem('localAgentWorkspace');
    if (savedWorkspace) {
      try { this.workspace = await invoke('set_workspace', { path: savedWorkspace }); }
      catch { localStorage.removeItem('localAgentWorkspace'); }
    }
    await this.refreshOllama();
  },
  toast(message) {
    this.notice = String(message);
    clearTimeout(this.noticeTimer);
    this.noticeTimer = setTimeout(() => { this.notice = ''; }, 4500);
  },
  async refreshOllama() {
    try {
      this.ollama = await invoke('ollama_status');
      if (!this.ollama.models.includes(this.model)) this.model = this.ollama.models[0] || '';
    } catch (error) { this.toast(error); }
  },
  async openOllama() {
    try { await openUrl('https://ollama.com/download'); }
    catch (error) { this.toast(error); }
  },
  async copy(value) {
    try { await navigator.clipboard.writeText(value); this.toast('Copied'); }
    catch (error) { this.toast(error); }
  },
  async chooseWorkspace() {
    try {
      const path = await open({ directory: true, multiple: false, title: 'Choose agent workspace' });
      if (!path) return;
      this.workspace = await invoke('set_workspace', { path });
      localStorage.setItem('localAgentWorkspace', this.workspace);
      this.messages = [];
      this.toast('Working folder updated');
    } catch (error) { this.toast(error); }
  },
  newConversation() {
    if (this.busy) return;
    this.messages = [];
    this.prompt = '';
    this.skillsOpen = false;
    this.view = 'chat';
  },
  async send() {
    if (this.busy || !this.workspace || !this.prompt.trim()) return;
    const content = this.prompt.trim();
    const history = this.messages.map((item) => ({
      role: item.role,
      content: `${item.content}${(item.results || []).map((result) => `\n${result.action} (${result.detail}): ${result.status}\n${result.output}`).join('')}`.slice(0, 8000),
    }));
    this.messages.push({ role: 'user', content, results: [] });
    this.prompt = '';
    this.busy = true;
    this.busyLabel = 'Thinking';
    this.scrollMessages();
    try {
      const outcome = await runAgentTurn({
        prompt: content,
        history,
        ask: (nextPrompt, nextHistory) => invoke('ask_agent', { prompt: nextPrompt, model: this.model, history: nextHistory }),
        execute: (plan) => invoke('execute_plan', { id: plan.id, confirmDestructive: true }),
        onPhase: (phase) => { this.busyLabel = phase; this.scrollMessages(); },
      });
      this.messages.push({
        role: 'assistant',
        content: outcome.message,
        results: outcome.results,
      });
    } catch (error) {
      this.messages.push({ role: 'assistant', content: `I could not complete the request: ${error}`, results: [] });
    } finally {
      this.busy = false;
      this.scrollMessages();
    }
  },
  scrollMessages() {
    this.$nextTick(() => {
      if (this.$refs.messages) this.$refs.messages.scrollTop = this.$refs.messages.scrollHeight;
    });
  },
}));

Alpine.start();
