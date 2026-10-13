// Supplemental focused decompilation; deliberately skips whole-program analysis.
// @category TobiiResearch
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import java.nio.file.Files;
import java.nio.file.Path;

public class HeadposeFocused extends GhidraScript {
    public void run() throws Exception {
        String[] args = getScriptArgs();
        DecompInterface decompiler = new DecompInterface();
        decompiler.openProgram(currentProgram);
        StringBuilder output = new StringBuilder("Supplemental Ghidra -noanalysis output; not REA Evidence.\n");
        try {
            for (int i = 1; i < args.length; i++) {
                Address address = toAddr(args[i]);
                disassemble(address);
                Function function = getFunctionAt(address);
                if (function == null) function = createFunction(address, null);
                output.append("\nADDRESS ").append(args[i]).append("\n");
                if (function == null) { output.append("FUNCTION UNRESOLVED\n"); continue; }
                DecompileResults result = decompiler.decompileFunction(function, 60, monitor);
                output.append(result.decompileCompleted() ? result.getDecompiledFunction().getC() : result.getErrorMessage());
                Files.writeString(Path.of(args[0]), output.toString());
                println("Completed focused function " + args[i]);
            }
            Files.writeString(Path.of(args[0]), output.toString());
        } finally { decompiler.dispose(); }
    }
}
