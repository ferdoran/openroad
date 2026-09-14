// Bulk corpus extractor for the quarantined vSRO Ghidra project.
//
// Idea: a Ghidra project can only be opened by one headless run at a time, so the
// expensive whole-program facts are dumped once into flat files that many analysis
// agents can grep in parallel without touching Ghidra again.
//
// Args: <outDir> <mode> [listFile]
//   index      -> functions.tsv, strings.tsv, srcpaths.tsv
//   decompile  -> one <addr>_<name>.c per line of listFile (lines: hex addr, '#' comments)
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
import ghidra.program.model.symbol.Reference;
import ghidra.program.model.symbol.ReferenceIterator;

import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.List;

public class ReDump extends GhidraScript {

    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        Path outDir = Paths.get(args[0]);
        Files.createDirectories(outDir);
        String mode = args[1];
        if ("index".equals(mode)) {
            dumpFunctions(outDir);
            dumpStrings(outDir);
        } else if ("decompile".equals(mode)) {
            bulkDecompile(outDir, Paths.get(args[2]));
        } else {
            println("ERROR: unknown mode " + mode);
        }
        println("DUMP-DONE");
    }

    private void dumpFunctions(Path outDir) throws Exception {
        try (PrintWriter pw = new PrintWriter(outDir.resolve("functions.tsv").toFile(), "UTF-8")) {
            pw.println("addr\tname\tsize\tcallers");
            FunctionIterator it = currentProgram.getFunctionManager().getFunctions(true);
            int n = 0;
            while (it.hasNext()) {
                Function f = it.next();
                int callers = 0;
                ReferenceIterator ri = currentProgram.getReferenceManager().getReferencesTo(f.getEntryPoint());
                while (ri.hasNext() && callers < 10000) { ri.next(); callers++; }
                pw.println(f.getEntryPoint() + "\t" + f.getName() + "\t" + f.getBody().getNumAddresses() + "\t" + callers);
                if (++n % 5000 == 0) println("funcs " + n);
            }
            println("FUNCS=" + n);
        }
    }

    private void dumpStrings(Path outDir) throws Exception {
        PrintWriter sp = new PrintWriter(outDir.resolve("strings.tsv").toFile(), "UTF-8");
        PrintWriter pp = new PrintWriter(outDir.resolve("srcpaths.tsv").toFile(), "UTF-8");
        sp.println("addr\tnrefs\treffuncs\tvalue");
        pp.println("addr\tnrefs\treffuncs\tvalue");
        DataIterator it = currentProgram.getListing().getDefinedData(true);
        int n = 0;
        while (it.hasNext()) {
            Data d = it.next();
            if (d == null || !d.hasStringValue()) continue;
            Object v = d.getValue();
            String s = (v instanceof StringDataInstance) ? ((StringDataInstance) v).getStringValue() : String.valueOf(v);
            if (s == null || s.length() < 3) continue;
            StringBuilder funcs = new StringBuilder();
            int nrefs = 0;
            ReferenceIterator ri = currentProgram.getReferenceManager().getReferencesTo(d.getAddress());
            while (ri.hasNext() && nrefs < 200) {
                Reference r = ri.next();
                nrefs++;
                if (nrefs <= 8) {
                    Function fn = getFunctionContaining(r.getFromAddress());
                    if (funcs.length() > 0) funcs.append(",");
                    funcs.append(fn == null ? r.getFromAddress().toString() : fn.getEntryPoint().toString());
                }
            }
            String esc = s.replace("\\", "\\\\").replace("\t", "\\t").replace("\r", "\\r").replace("\n", "\\n");
            String line = d.getAddress() + "\t" + nrefs + "\t" + funcs + "\t" + esc;
            sp.println(line);
            String low = s.toLowerCase();
            if (low.contains(".cpp") || low.contains(".h") || low.contains("vss-od") || low.contains("\\silkroad\\")) {
                pp.println(line);
            }
            if (++n % 20000 == 0) println("strings " + n);
        }
        sp.close();
        pp.close();
        println("STRINGS=" + n);
    }

    private void bulkDecompile(Path outDir, Path listFile) throws Exception {
        DecompInterface dec = new DecompInterface();
        dec.setOptions(new DecompileOptions());
        dec.openProgram(currentProgram);
        List<String> lines = Files.readAllLines(listFile, StandardCharsets.UTF_8);
        int n = 0, ok = 0;
        for (String raw : lines) {
            String t = raw.trim();
            if (t.isEmpty() || t.startsWith("#")) continue;
            if (t.toLowerCase().startsWith("0x")) t = t.substring(2);
            n++;
            Address a;
            try {
                a = currentProgram.getAddressFactory().getDefaultAddressSpace().getAddress(Long.parseLong(t, 16));
            } catch (Exception e) { continue; }
            Function f = getFunctionAt(a);
            if (f == null) f = getFunctionContaining(a);
            // Registration tables point at raw LAB_ addresses Ghidra never turned into
            // functions; define one in-memory so the decompiler has an entry point.
            if (f == null) {
                try { f = createFunction(a, null); } catch (Exception ignored) { }
            }
            if (f == null) { println("NOFUNC " + a); continue; }
            Path out = outDir.resolve(f.getEntryPoint() + "_" + f.getName() + ".c");
            if (Files.exists(out)) { ok++; continue; }
            DecompileResults res = dec.decompileFunction(f, 90, monitor);
            String c = (res != null && res.decompileCompleted() && res.getDecompiledFunction() != null)
                    ? res.getDecompiledFunction().getC()
                    : ("/* decompile failed */");
            try (PrintWriter pw = new PrintWriter(out.toFile(), "UTF-8")) {
                pw.println("/* " + currentProgram.getName() + " @ " + f.getEntryPoint()
                        + "  size=" + f.getBody().getNumAddresses() + " */");
                pw.print(c);
            }
            ok++;
            if (ok % 25 == 0) println("decompiled " + ok + "/" + n);
        }
        dec.dispose();
        println("DECOMPILED=" + ok + " of " + n);
    }
}
