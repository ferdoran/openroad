// Batch RE query runner for the quarantined vSRO Ghidra project.
//
// Idea: a Ghidra headless startup costs ~30s, so one run must answer many questions.
// This script reads a job file (one query per line, pipe-separated) and writes a single
// JSON result file. Static reads only - decompile, bytes, strings, xrefs, disasm - never
// executes anything from the analysed image.
//
// Job line grammar (one per line, '#' starts a comment):
//   id|decompile|<addrOrName>
//   id|bytes|<addr>|<len>
//   id|strings|<javaRegex>|<limit>
//   id|xrefs|<addr>
//   id|funcs|<javaRegexOverName>|<limit>
//   id|callers|<addrOrName>
//   id|callees|<addrOrName>
//   id|disasm|<addr>|<instrCount>
//   id|data|<addr>
//
// Args: <jobFile> <outJsonFile>
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileOptions;
import ghidra.app.decompiler.DecompileResults;
import ghidra.program.model.address.Address;
import ghidra.program.model.data.StringDataInstance;
import ghidra.program.model.listing.Data;
import ghidra.program.model.listing.DataIterator;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionIterator;
import ghidra.program.model.listing.Instruction;
import ghidra.program.model.symbol.Reference;
import ghidra.program.model.symbol.ReferenceIterator;

import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.List;
import java.util.regex.Pattern;

public class ReQuery extends GhidraScript {

    private DecompInterface decomp;

    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length < 2) {
            println("ERROR: need <jobFile> <outFile>");
            return;
        }
        List<String> lines = Files.readAllLines(Paths.get(args[0]), StandardCharsets.UTF_8);
        StringBuilder out = new StringBuilder();
        out.append("{\"program\":").append(q(currentProgram.getName()))
           .append(",\"results\":[");
        boolean first = true;
        for (String raw : lines) {
            String line = raw.trim();
            if (line.isEmpty() || line.startsWith("#")) continue;
            String[] p = line.split("\\|", -1);
            if (p.length < 2) continue;
            String id = p[0], op = p[1];
            if (!first) out.append(",");
            first = false;
            out.append("{\"id\":").append(q(id)).append(",\"op\":").append(q(op)).append(",");
            try {
                switch (op) {
                    case "decompile": out.append(decompile(p[2])); break;
                    case "bytes":     out.append(bytes(p[2], Integer.parseInt(p[3]))); break;
                    case "strings":   out.append(strings(p[2], p.length > 3 ? Integer.parseInt(p[3]) : 200)); break;
                    case "xrefs":     out.append(xrefs(p[2])); break;
                    case "funcs":     out.append(funcs(p[2], p.length > 3 ? Integer.parseInt(p[3]) : 200)); break;
                    case "callers":   out.append(calls(p[2], true)); break;
                    case "callees":   out.append(calls(p[2], false)); break;
                    case "disasm":    out.append(disasm(p[2], Integer.parseInt(p[3]))); break;
                    case "data":      out.append(dataAt(p[2])); break;
                    default:          out.append("\"error\":\"unknown op\"");
                }
            } catch (Exception e) {
                out.append("\"error\":").append(q(e.getClass().getSimpleName() + ": " + e.getMessage()));
            }
            out.append("}");
            println("DONE " + id + " " + op);
        }
        out.append("]}");
        try (PrintWriter pw = new PrintWriter(args[1], "UTF-8")) {
            pw.print(out);
        }
        if (decomp != null) decomp.dispose();
        println("WROTE " + args[1]);
    }

    // ---- ops -------------------------------------------------------------

    private String decompile(String target) throws Exception {
        Function f = resolve(target);
        if (f == null) return "\"error\":\"function not found\"";
        if (decomp == null) {
            decomp = new DecompInterface();
            DecompileOptions opts = new DecompileOptions();
            decomp.setOptions(opts);
            decomp.openProgram(currentProgram);
        }
        DecompileResults res = decomp.decompileFunction(f, 120, monitor);
        String c = (res != null && res.decompileCompleted() && res.getDecompiledFunction() != null)
                ? res.getDecompiledFunction().getC()
                : ("/* decompile failed: " + (res == null ? "null" : res.getErrorMessage()) + " */");
        return "\"name\":" + q(f.getName())
             + ",\"addr\":" + q(f.getEntryPoint().toString())
             + ",\"signature\":" + q(f.getPrototypeString(true, true))
             + ",\"size\":" + f.getBody().getNumAddresses()
             + ",\"c\":" + q(c);
    }

    private String bytes(String addrStr, int len) throws Exception {
        Address a = addr(addrStr);
        byte[] b = new byte[len];
        currentProgram.getMemory().getBytes(a, b);
        StringBuilder hex = new StringBuilder();
        for (byte x : b) hex.append(String.format("%02x", x));
        StringBuilder sb = new StringBuilder();
        sb.append("\"addr\":").append(q(a.toString())).append(",\"hex\":").append(q(hex.toString()));
        if (len >= 4) {
            int i32 = (b[0] & 0xff) | ((b[1] & 0xff) << 8) | ((b[2] & 0xff) << 16) | ((b[3] & 0xff) << 24);
            sb.append(",\"u32_le\":").append(Integer.toUnsignedString(i32));
            sb.append(",\"i32_le\":").append(i32);
            sb.append(",\"f32_le\":").append(q(String.valueOf(Float.intBitsToFloat(i32))));
        }
        if (len >= 8) {
            long lo = 0;
            for (int i = 7; i >= 0; i--) lo = (lo << 8) | (b[i] & 0xffL);
            sb.append(",\"u64_le\":").append(Long.toUnsignedString(lo));
            sb.append(",\"f64_le\":").append(q(String.valueOf(Double.longBitsToDouble(lo))));
        }
        StringBuilder ascii = new StringBuilder();
        for (byte x : b) ascii.append((x >= 32 && x < 127) ? (char) x : '.');
        sb.append(",\"ascii\":").append(q(ascii.toString()));
        return sb.toString();
    }

    private String strings(String regex, int limit) {
        Pattern pat = Pattern.compile(regex);
        StringBuilder sb = new StringBuilder("\"hits\":[");
        int n = 0;
        boolean first = true;
        DataIterator it = currentProgram.getListing().getDefinedData(true);
        while (it.hasNext() && n < limit) {
            Data d = it.next();
            if (d == null || !d.hasStringValue()) continue;
            Object v = d.getValue();
            String s = (v instanceof StringDataInstance) ? ((StringDataInstance) v).getStringValue() : String.valueOf(v);
            if (s == null || !pat.matcher(s).find()) continue;
            n++;
            if (!first) sb.append(",");
            first = false;
            sb.append("{\"addr\":").append(q(d.getAddress().toString()))
              .append(",\"s\":").append(q(s))
              .append(",\"refs\":[");
            boolean rf = true;
            ReferenceIterator ri = currentProgram.getReferenceManager().getReferencesTo(d.getAddress());
            int rc = 0;
            while (ri.hasNext() && rc < 40) {
                Reference r = ri.next();
                rc++;
                Function fn = getFunctionContaining(r.getFromAddress());
                if (!rf) sb.append(",");
                rf = false;
                sb.append("{\"from\":").append(q(r.getFromAddress().toString()))
                  .append(",\"func\":").append(q(fn == null ? "" : fn.getName()))
                  .append(",\"func_addr\":").append(q(fn == null ? "" : fn.getEntryPoint().toString()))
                  .append("}");
            }
            sb.append("]}");
        }
        sb.append("],\"count\":").append(n);
        return sb.toString();
    }

    private String xrefs(String addrStr) throws Exception {
        Address a = addr(addrStr);
        StringBuilder sb = new StringBuilder("\"refs\":[");
        boolean first = true;
        int n = 0;
        ReferenceIterator ri = currentProgram.getReferenceManager().getReferencesTo(a);
        while (ri.hasNext() && n < 500) {
            Reference r = ri.next();
            n++;
            Function fn = getFunctionContaining(r.getFromAddress());
            if (!first) sb.append(",");
            first = false;
            sb.append("{\"from\":").append(q(r.getFromAddress().toString()))
              .append(",\"type\":").append(q(r.getReferenceType().getName()))
              .append(",\"func\":").append(q(fn == null ? "" : fn.getName()))
              .append(",\"func_addr\":").append(q(fn == null ? "" : fn.getEntryPoint().toString()))
              .append("}");
        }
        sb.append("],\"count\":").append(n);
        return sb.toString();
    }

    private String funcs(String regex, int limit) {
        Pattern pat = Pattern.compile(regex);
        StringBuilder sb = new StringBuilder("\"funcs\":[");
        boolean first = true;
        int n = 0;
        FunctionIterator it = currentProgram.getFunctionManager().getFunctions(true);
        while (it.hasNext() && n < limit) {
            Function f = it.next();
            if (!pat.matcher(f.getName()).find()) continue;
            n++;
            if (!first) sb.append(",");
            first = false;
            sb.append("{\"name\":").append(q(f.getName()))
              .append(",\"addr\":").append(q(f.getEntryPoint().toString()))
              .append(",\"size\":").append(f.getBody().getNumAddresses())
              .append("}");
        }
        sb.append("],\"count\":").append(n);
        return sb.toString();
    }

    private String calls(String target, boolean incoming) throws Exception {
        Function f = resolve(target);
        if (f == null) return "\"error\":\"function not found\"";
        java.util.Set<Function> set = incoming ? f.getCallingFunctions(monitor) : f.getCalledFunctions(monitor);
        StringBuilder sb = new StringBuilder("\"funcs\":[");
        boolean first = true;
        for (Function g : set) {
            if (!first) sb.append(",");
            first = false;
            sb.append("{\"name\":").append(q(g.getName()))
              .append(",\"addr\":").append(q(g.getEntryPoint().toString()))
              .append("}");
        }
        sb.append("],\"count\":").append(set.size());
        return sb.toString();
    }

    private String disasm(String addrStr, int count) throws Exception {
        Address a = addr(addrStr);
        StringBuilder sb = new StringBuilder("\"instrs\":[");
        Instruction ins = getInstructionAt(a);
        if (ins == null) ins = getInstructionAfter(a);
        boolean first = true;
        for (int i = 0; i < count && ins != null; i++) {
            if (!first) sb.append(",");
            first = false;
            sb.append("{\"addr\":").append(q(ins.getAddress().toString()))
              .append(",\"text\":").append(q(ins.toString()))
              .append("}");
            ins = ins.getNext();
        }
        sb.append("]");
        return sb.toString();
    }

    private String dataAt(String addrStr) throws Exception {
        Address a = addr(addrStr);
        Data d = getDataAt(a);
        if (d == null) d = getDataContaining(a);
        if (d == null) return "\"error\":\"no defined data\"";
        return "\"addr\":" + q(d.getAddress().toString())
             + ",\"type\":" + q(d.getDataType().getName())
             + ",\"len\":" + d.getLength()
             + ",\"repr\":" + q(String.valueOf(d.getDefaultValueRepresentation()));
    }

    // ---- helpers ---------------------------------------------------------

    private Function resolve(String target) {
        String t = target.trim();
        if (t.matches("(?i)^(0x)?[0-9a-f]{5,16}$")) {
            Address a = addr(t);
            Function f = getFunctionAt(a);
            if (f == null) f = getFunctionContaining(a);
            if (f != null) return f;
        }
        List<Function> fs = getGlobalFunctions(t);
        if (fs != null && !fs.isEmpty()) return fs.get(0);
        // fall back: FUN_xxxxxxxx style name -> address
        if (t.toUpperCase().startsWith("FUN_")) {
            Address a = addr(t.substring(4));
            Function f = getFunctionAt(a);
            if (f != null) return f;
        }
        return null;
    }

    private Address addr(String s) {
        String t = s.trim();
        if (t.toLowerCase().startsWith("0x")) t = t.substring(2);
        return currentProgram.getAddressFactory().getDefaultAddressSpace().getAddress(Long.parseLong(t, 16));
    }

    private static String q(String s) {
        if (s == null) return "\"\"";
        StringBuilder sb = new StringBuilder("\"");
        for (int i = 0; i < s.length(); i++) {
            char c = s.charAt(i);
            switch (c) {
                case '"': sb.append("\\\""); break;
                case '\\': sb.append("\\\\"); break;
                case '\n': sb.append("\\n"); break;
                case '\r': sb.append("\\r"); break;
                case '\t': sb.append("\\t"); break;
                default:
                    if (c < 0x20 || c > 0x7e) sb.append(String.format("\\u%04x", (int) c));
                    else sb.append(c);
            }
        }
        return sb.append("\"").toString();
    }
}
