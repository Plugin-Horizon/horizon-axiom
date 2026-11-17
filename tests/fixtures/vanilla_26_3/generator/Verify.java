import net.minecraft.core.IdMapper;
import net.minecraft.network.FriendlyByteBuf;
import net.minecraft.world.level.chunk.PalettedContainer;
import net.minecraft.world.level.chunk.Strategy;
import io.netty.buffer.Unpooled;
import java.nio.file.*;

/** Reads Rust-written sections with vanilla 26.3's PalettedContainer.read and checks every cell. */
public class Verify {
    interface F { int at(int i); }
    public static void main(String[] a) throws Exception {
        int n = 30000;
        Integer[] objs = new Integer[n];
        IdMapper<Integer> map = new IdMapper<>(n);
        for (int i = 0; i < n; i++) { objs[i] = new Integer(i); map.add(objs[i]); }
        Strategy<Integer> strategy = Strategy.createForBlockStates(map);
        Object[][] cases = {
            {"single", (F) i -> 7}, {"three", (F) i -> (i % 3) * 100}, {"seventeen", (F) i -> 100 + i % 17},
            {"distinct255", (F) i -> i % 255}, {"global300", (F) i -> 1000 + i % 300}, {"global_all", (F) i -> (i * 7) % 20000},
        };
        int bad = 0;
        for (Object[] c : cases) {
            byte[] b = Files.readAllBytes(Paths.get(a[0], c[0] + ".bin"));
            FriendlyByteBuf buf = new FriendlyByteBuf(Unpooled.wrappedBuffer(b));
            PalettedContainer<Integer> pc = new PalettedContainer<>(objs[0], strategy);
            pc.read(buf);
            F f = (F) c[1];
            int wrong = 0;
            for (int i = 0; i < 4096; i++) if (pc.get(i & 15, i >> 8, (i >> 4) & 15).intValue() != f.at(i)) wrong++;
            boolean ok = wrong == 0 && buf.readableBytes() == 0;
            if (!ok) bad++;
            System.out.println(c[0] + ": " + (ok ? "OK" : "FAIL wrong=" + wrong + " left=" + buf.readableBytes()));
        }
        System.exit(bad == 0 ? 0 : 1);
    }
}
