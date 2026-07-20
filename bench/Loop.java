public class Loop {
    public static void main(String[] a) {
        long s = 0, i = 0;
        while (i < 10000000L) { s = (s + i) % 1000003L; i = i + 1; }
        System.out.println(s);
    }
}
