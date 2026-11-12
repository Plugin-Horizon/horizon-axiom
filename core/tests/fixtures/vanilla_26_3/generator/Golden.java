import net.minecraft.core.IdMapper;
import net.minecraft.network.FriendlyByteBuf;
import net.minecraft.world.level.chunk.PalettedContainer;
import net.minecraft.world.level.chunk.Strategy;
import io.netty.buffer.Unpooled;
import java.nio.file.*;

/** Writes sections with vanilla 26.3's PalettedContainer (block-state strategy, 2^15 - 1000 states). */
public class Golden {
    interface F { int at(int i); }
    public static void main(String[] a) throws Exception {
        int n = 30000; // a global palette of 15 bits, like the real block state count
        Integer[] objs = new Integer[n];
        IdMapper<Integer> map = new IdMapper<>(n);
        for (int i = 0; i < n; i++) { objs[i] = new Integer(i); map.add(objs[i]); }
        Strategy<Integer> strategy = Strategy.createForBlockStates(map);
        Object[][] cases = {
            {"single", (F) i -> 7},
            {"three", (F) i -> (i % 3) * 100},
            {"seventeen", (F) i -> 100 + i % 17},
            {"distinct255", (F) i -> i % 255},
            {"global300", (F) i -> 1000 + i % 300},
            {"global_all", (F) i -> (i * 7) % 20000},
        };
        Path out = Paths.get(a[0]);
        for (Object[] c : cases) {
            PalettedContainer<Integer> pc = new PalettedContainer<>(objs[0], strategy);
            F f = (F) c[1];
            for (int i = 0; i < 4096; i++) pc.set(i & 15, i >> 8, (i >> 4) & 15, objs[f.at(i)]);
            FriendlyByteBuf buf = new FriendlyByteBuf(Unpooled.buffer());
            pc.write(buf);
            byte[] b = new byte[buf.readableBytes()];
            buf.readBytes(b);
            Files.write(out.resolve(c[0] + ".bin"), b);
            System.out.println(c[0] + " " + b.length + " bytes, bits " + b[0]);
        }
    }
}
