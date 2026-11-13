import {Context, run} from "cordis-sdk";
import * as binding from "../text-services.js";

async function apply(context: Context): Promise<void> {
    await context.provide("text-length@1.0", {
        measure(value: string): number { return Array.from(value).length; },
    }, binding);
}

void run("web-typescript-service", [{name: "length", apply, provides: ["text-length@1.0"]}]);
