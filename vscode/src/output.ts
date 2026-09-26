import * as vscode from 'vscode';

let outputChannel: vscode.OutputChannel | undefined;

export function initOutputChannel(): vscode.OutputChannel {
  outputChannel = vscode.window.createOutputChannel('dcmview');
  return outputChannel;
}

export function getOutputChannel(): vscode.OutputChannel {
  if (!outputChannel) {
    outputChannel = vscode.window.createOutputChannel('dcmview');
  }
  return outputChannel;
}

export function formatError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
